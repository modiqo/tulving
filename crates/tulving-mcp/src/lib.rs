//! Stateless MCP server over stdio: the agent surface of tulving.
//!
//! Targets MCP 2026-07-28, whose core is stateless — no
//! `initialize`/`initialized` handshake, requests self-describing, and
//! the optional `server/discover` call for up-front capability reads.
//! Deployed clients still open stdio servers with `initialize`, so this
//! server answers it too and negotiates down to their version. Either
//! way every request stands alone: each call opens the ledger fresh and
//! no session state exists between messages.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{json, Value};
use tulving_core::{cadence, ops, Ledger, ScheduleSpec};

/// Protocol revision this server speaks natively.
const PROTOCOL_LATEST: &str = "2026-07-28";
/// Older revisions the server accepts from `initialize`-style clients.
const PROTOCOL_COMPAT: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// Cache lifetime advertised on list results (`ttlMs`, spec 2026-07-28).
const LIST_TTL_MS: u64 = 300_000;

/// Run the stdio server: newline-delimited JSON-RPC 2.0 on stdin/stdout.
/// Returns when stdin closes.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(err) => {
                let reply = error_reply(Value::Null, -32700, &format!("parse error: {err}"));
                writeln!(stdout, "{reply}")?;
                stdout.flush()?;
                continue;
            }
        };
        if let Some(reply) = handle(&message) {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Dispatch one JSON-RPC message. Notifications return `None`.
fn handle(message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str)?;
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // Notifications (no id) get no reply, per JSON-RPC 2.0.
    let id = match id {
        Some(id) if !id.is_null() => id,
        _ => return None,
    };

    let result = match method {
        "initialize" => Ok(initialize_result(&params)),
        "server/discover" => Ok(discover_result()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => tools_call(&params),
        "prompts/list" => Ok(json!({
            "prompts": [], "ttlMs": LIST_TTL_MS, "cacheScope": "private"
        })),
        "resources/list" => Ok(json!({
            "resources": [], "ttlMs": LIST_TTL_MS, "cacheScope": "private"
        })),
        _ => {
            return Some(error_reply(
                id,
                -32601,
                &format!("method '{method}' not found"),
            ));
        }
    };

    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err(err) => error_reply(id, -32603, &format!("{err:#}")),
    })
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// Legacy handshake support: echo a version the client knows, or offer
/// the latest. No state is created by answering this.
fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_LATEST);
    let version = if requested == PROTOCOL_LATEST || PROTOCOL_COMPAT.contains(&requested) {
        requested
    } else {
        PROTOCOL_LATEST
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
    })
}

/// The 2026-07-28 replacement for capability negotiation: optional,
/// idempotent, and answerable without any prior message.
fn discover_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_LATEST,
        "capabilities": { "tools": {} },
        "serverInfo": server_info(),
    })
}

fn server_info() -> Value {
    json!({
        "name": "tulving",
        "title": "tulving — a tiny cron with a memory",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Tool catalog. Names and shapes mirror the CLI verbs one-to-one so a
/// harness that learned one surface has learned both.
fn tools_list() -> Value {
    let string = |desc: &str| json!({ "type": "string", "description": desc });
    let tools = json!([
        {
            "name": "schedule",
            "title": "Crystallize a schedule",
            "description": "Schedule a command that just worked. Cadence is plain \
                words ('every morning', '15m', 'weekdays 7:30'). Store the why. \
                Prefer a stop condition: max_runs, for, or until.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "argv": { "type": "array", "items": { "type": "string" },
                              "description": "The command as an argv array, verbatim from the run that worked" },
                    "cadence": string("Plain-words cadence, e.g. 'every morning', '15m', 'weekdays 7:30'"),
                    "why": string("Why this schedule exists; shown by `why` and in digests"),
                    "max_runs": { "type": "integer", "description": "Retire after N runs" },
                    "for": string("Retire after a duration: 2w, 14d, 6h"),
                    "until": string("Retire when this jq predicate over the result is true; `prev` names the previous result"),
                    "on": string("Fire the notifier when this jq predicate is true, e.g. '.dau < prev.dau * 0.95'"),
                    "notify": { "type": "array", "items": { "type": "string" },
                                "description": "Notifier argv; receives the envelope JSON on stdin" },
                    "on_change": string("Diff results against the previous run; optional JSON pointer scopes it"),
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "origin": { "type": "object", "description": "Who is crystallizing: harness, session, pinned ref" }
                },
                "required": ["argv", "cadence"]
            },
            "outputSchema": { "type": "object", "description": "The stored schedule" }
        },
        {
            "name": "recall",
            "title": "Recall the ledger",
            "description": "Read run envelopes on a timescale. Call at session start \
                with since='last-session' semantics (e.g. '24h') to learn what \
                happened while you were away.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": string("'yesterday', 'today', '6h', '2d', or RFC 3339 (default '24h')"),
                    "changed": { "type": "boolean", "description": "Only runs whose result moved" },
                    "failed": { "type": "boolean", "description": "Only failed runs" },
                    "schedule": string("Limit to one schedule id")
                }
            },
            "outputSchema": {
                "type": "object",
                "properties": { "envelopes": { "type": "array" } }
            }
        },
        {
            "name": "schedules",
            "title": "List schedules",
            "description": "List schedules: id, cadence, next run, run count, mortality.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "all": { "type": "boolean", "description": "Include retired schedules" }
                }
            },
            "outputSchema": {
                "type": "object",
                "properties": { "schedules": { "type": "array" } }
            }
        },
        {
            "name": "why",
            "title": "Why does this schedule exist",
            "description": "The stored intent, origin, and lifecycle of one schedule.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": string("Schedule id") },
                "required": ["id"]
            },
            "outputSchema": { "type": "object" }
        },
        {
            "name": "run_now",
            "title": "Run a schedule now",
            "description": "Execute one schedule immediately and return its envelope.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": string("Schedule id") },
                "required": ["id"]
            },
            "outputSchema": { "type": "object", "description": "The run envelope" }
        },
        {
            "name": "retire",
            "title": "Retire a schedule",
            "description": "End a schedule. Its envelopes stay in the ledger.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": string("Schedule id") },
                "required": ["id"]
            },
            "outputSchema": { "type": "object" }
        }
    ]);
    json!({ "tools": tools, "ttlMs": LIST_TTL_MS, "cacheScope": "private" })
}

/// Execute one tool call. Tool-level failures return an in-band
/// `isError` result, not a protocol error, per the MCP tool contract.
fn tools_call(params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match dispatch_tool(name, &args) {
        Ok((text, structured)) => Ok(json!({
            "content": [ { "type": "text", "text": text } ],
            "structuredContent": structured,
            "isError": false
        })),
        Err(err) => Ok(json!({
            "content": [ { "type": "text", "text": format!("{err:#}") } ],
            "isError": true
        })),
    }
}

fn dispatch_tool(name: &str, args: &Value) -> Result<(String, Value)> {
    let ledger = Ledger::open_default()?;
    match name {
        "schedule" => tool_schedule(&ledger, args),
        "recall" => tool_recall(&ledger, args),
        "schedules" => tool_schedules(&ledger, args),
        "why" => tool_why(&ledger, args),
        "run_now" => tool_run_now(&ledger, args),
        "retire" => tool_retire(&ledger, args),
        other => anyhow::bail!("unknown tool '{other}'"),
    }
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn arg_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn required_id(args: &Value) -> Result<String> {
    arg_str(args, "id").ok_or_else(|| anyhow::anyhow!("missing required argument 'id'"))
}

fn tool_schedule(ledger: &Ledger, args: &Value) -> Result<(String, Value)> {
    let argv = arg_vec(args, "argv");
    let cadence_text = arg_str(args, "cadence")
        .ok_or_else(|| anyhow::anyhow!("missing required argument 'cadence'"))?;
    let notify = args
        .get("notify")
        .and_then(Value::as_array)
        .map(|_| arg_vec(args, "notify"));
    let spec = ScheduleSpec {
        argv,
        cadence: cadence_text,
        why: arg_str(args, "why"),
        cwd: None,
        env: None,
        origin: Some(
            args.get("origin")
                .cloned()
                .unwrap_or_else(|| json!({ "harness": "mcp" })),
        ),
        max_runs: args.get("max_runs").and_then(Value::as_i64),
        expires_at: None,
        for_: arg_str(args, "for"),
        until: arg_str(args, "until"),
        on: arg_str(args, "on"),
        notify,
        on_change: arg_str(args, "on_change"),
        tags: arg_vec(args, "tags"),
    };
    let schedule = ops::crystallize(ledger, spec)?;
    let mortality = match (schedule.max_runs, schedule.expires_at, &schedule.until) {
        (Some(n), _, _) => format!("after {n} runs"),
        (None, Some(t), _) => format!("at {t}"),
        (None, None, Some(p)) => format!("when {p}"),
        (None, None, None) => "never — consider max_runs, for, or until".into(),
    };
    let text = format!(
        "Scheduled #{} — runs {} (cron {}), dies {}.",
        schedule.id, schedule.cadence, schedule.cron, mortality
    );
    Ok((text, serde_json::to_value(schedule)?))
}

fn tool_recall(ledger: &Ledger, args: &Value) -> Result<(String, Value)> {
    let since_text = arg_str(args, "since").unwrap_or_else(|| "24h".into());
    let since = cadence::parse_since(&since_text)?;
    let changed = args
        .get("changed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let failed = args.get("failed").and_then(Value::as_bool).unwrap_or(false);
    let schedule = arg_str(args, "schedule");
    let envelopes = ledger.recall(since, changed, failed, schedule.as_deref())?;
    let moved = envelopes.iter().filter(|e| e.changed).count();
    let missed = envelopes.iter().filter(|e| e.missed).count();
    let text = format!(
        "{} envelope(s) since {since_text}: {moved} changed, {missed} missed.",
        envelopes.len()
    );
    Ok((
        text,
        json!({ "envelopes": serde_json::to_value(envelopes)? }),
    ))
}

fn tool_schedules(ledger: &Ledger, args: &Value) -> Result<(String, Value)> {
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let schedules = ledger.list_schedules(all)?;
    let text = format!("{} schedule(s).", schedules.len());
    Ok((
        text,
        json!({ "schedules": serde_json::to_value(schedules)? }),
    ))
}

fn tool_why(ledger: &Ledger, args: &Value) -> Result<(String, Value)> {
    let id = required_id(args)?;
    let schedule = ledger
        .get_schedule(&id)?
        .ok_or_else(|| anyhow::anyhow!("no schedule '{id}'"))?;
    let text = schedule
        .why
        .clone()
        .unwrap_or_else(|| "(no why recorded)".into());
    Ok((text, serde_json::to_value(schedule)?))
}

fn tool_run_now(ledger: &Ledger, args: &Value) -> Result<(String, Value)> {
    let id = required_id(args)?;
    let schedule = ledger
        .get_schedule(&id)?
        .ok_or_else(|| anyhow::anyhow!("no schedule '{id}'"))?;
    let envelope = ops::run_schedule(ledger, &schedule, false)?;
    let text = format!(
        "Ran #{id}: exit {:?}, changed: {}.",
        envelope.exit, envelope.changed
    );
    Ok((text, serde_json::to_value(envelope)?))
}

fn tool_retire(ledger: &Ledger, args: &Value) -> Result<(String, Value)> {
    let id = required_id(args)?;
    ledger.retire(&id, "retired via MCP")?;
    Ok((
        format!("Retired #{id}."),
        json!({ "id": id, "status": "retired" }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_needs_no_prior_message() {
        let reply = handle(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "server/discover"
        }))
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_LATEST);
    }

    #[test]
    fn initialize_negotiates_down_for_older_clients() {
        let reply = handle(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18" }
        }))
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn initialize_offers_latest_to_unknown_versions() {
        let reply = handle(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" }
        }))
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], PROTOCOL_LATEST);
    }

    #[test]
    fn tools_list_carries_cache_metadata() {
        let reply = handle(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/list"
        }))
        .unwrap();
        assert_eq!(reply["result"]["ttlMs"], LIST_TTL_MS);
        assert_eq!(reply["result"]["cacheScope"], "private");
        assert!(reply["result"]["tools"].as_array().unwrap().len() >= 6);
    }

    #[test]
    fn notifications_get_no_reply() {
        assert!(handle(&json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))
        .is_none());
    }

    #[test]
    fn unknown_method_is_a_json_rpc_error() {
        let reply = handle(&json!({
            "jsonrpc": "2.0", "id": 3, "method": "sessions/create"
        }))
        .unwrap();
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn bad_tool_call_is_in_band_not_protocol_error() {
        // Keep the test ledger away from the user's real ~/.tulving.
        let dir = std::env::temp_dir().join("tulving-mcp-test-home");
        std::env::set_var("TULVING_HOME", &dir);
        let reply = handle(&json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "schedule", "arguments": { "argv": [], "cadence": "hourly" } }
        }))
        .unwrap();
        assert_eq!(reply["result"]["isError"], true);
    }
}
