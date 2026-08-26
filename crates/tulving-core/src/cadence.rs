//! Plain-words cadence: the parser that keeps cron syntax out of sight.
//! "every morning" and "weekdays 7:30" normalize to 5-field cron for
//! storage; raw cron expressions pass through unchanged.

use anyhow::{bail, Context, Result};

/// Normalize plain-words cadence to a 5-field cron expression.
///
/// Grammar (case-insensitive, leading "every" optional):
///   morning                  -> weekdays 07:30
///   daily [HH:MM|Ham/pm]     -> every day at time (default 07:30)
///   hourly                   -> minute 0 of every hour
///   weekday[s] [time]        -> Mon-Fri at time (default 07:30)
///   <N>m / <N>h              -> interval minutes / hours
///   <dayname> [time]         -> that day weekly (default 09:00)
///   HH:MM                    -> daily at that time
/// A raw 5-field cron expression passes through unchanged.
pub fn to_cron(cadence: &str) -> Result<String> {
    let text = cadence.trim().to_lowercase();
    let text = text
        .strip_prefix("every ")
        .unwrap_or(&text)
        .trim()
        .to_string();
    let text = text
        .strip_prefix("each ")
        .unwrap_or(&text)
        .trim()
        .to_string();
    if text == "weekly" {
        return Ok("0 9 * * 1".into());
    }
    let text = text
        .strip_prefix("weekly ")
        .unwrap_or(&text)
        .trim()
        .to_string();
    if text.is_empty() {
        bail!("empty cadence");
    }

    // Raw cron passthrough: five whitespace-separated fields.
    if text.split_whitespace().count() == 5
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || " */,-".contains(c))
    {
        return Ok(text);
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    let Some((&head, rest)) = words.split_first() else {
        bail!("empty cadence");
    };

    // Intervals: 15m, 5h (also "15 m").
    if let Some(cron) = interval(&text)? {
        return Ok(cron);
    }

    match head {
        "morning" => return Ok("30 7 * * 1-5".into()),
        "hourly" => return Ok("0 * * * *".into()),
        "daily" | "day" => {
            let (h, m) = time_of(rest).unwrap_or((7, 30));
            return Ok(format!("{m} {h} * * *"));
        }
        "weekday" | "weekdays" => {
            let (h, m) = time_of(rest).unwrap_or((7, 30));
            return Ok(format!("{m} {h} * * 1-5"));
        }
        _ => {}
    }

    if let Some(dow) = day_number(head) {
        let (h, m) = time_of(rest).unwrap_or((9, 0));
        return Ok(format!("{m} {h} * * {dow}"));
    }

    if let Some((h, m)) = parse_time(head) {
        return Ok(format!("{m} {h} * * *"));
    }

    bail!(
        "cannot understand cadence '{cadence}'; try 'morning', 'daily 9am', \
         'weekdays 7:30', 'every 15m', 'monday 9am', or a 5-field cron"
    )
}

fn interval(text: &str) -> Result<Option<String>> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(unit) = compact.chars().last() else {
        return Ok(None);
    };
    let Some(body) = compact.strip_suffix(unit) else {
        return Ok(None);
    };
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    let n: u32 = body.parse()?;
    match unit {
        'm' => {
            if n == 0 || n > 59 {
                bail!("minute interval must be 1-59, got {n}");
            }
            Ok(Some(format!("*/{n} * * * *")))
        }
        'h' => {
            if n == 0 || n > 23 {
                bail!("hour interval must be 1-23, got {n}");
            }
            Ok(Some(format!("0 */{n} * * *")))
        }
        _ => Ok(None),
    }
}

fn day_number(word: &str) -> Option<u8> {
    let w = word.trim_end_matches('s');
    match w {
        "sunday" | "sun" => Some(0),
        "monday" | "mon" => Some(1),
        "tuesday" | "tue" => Some(2),
        "wednesday" | "wed" => Some(3),
        "thursday" | "thu" => Some(4),
        "friday" | "fri" => Some(5),
        "saturday" | "sat" => Some(6),
        _ => None,
    }
}

fn time_of(words: &[&str]) -> Option<(u32, u32)> {
    let joined = words.join("");
    let joined = joined.strip_prefix("at").unwrap_or(&joined);
    parse_time(joined)
}

/// "7:30", "07:30", "9am", "9pm", "21".
fn parse_time(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (body, pm) = if let Some(b) = s.strip_suffix("pm") {
        (b, true)
    } else if let Some(b) = s.strip_suffix("am") {
        (b, false)
    } else {
        (s, false)
    };
    let (h_str, m_str) = match body.split_once(':') {
        Some((h, m)) => (h, m),
        None => (body, "0"),
    };
    let mut h: u32 = h_str.trim().parse().ok()?;
    let m: u32 = m_str.trim().parse().ok()?;
    if pm && h < 12 {
        h += 12;
    }
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

/// "2w", "14d", "6h", "30m" -> chrono::Duration for `--for`.
pub fn parse_duration(s: &str) -> Result<chrono::Duration> {
    let s = s.trim().to_lowercase();
    let Some(unit) = s.chars().last() else {
        bail!("empty duration")
    };
    let body = s.strip_suffix(unit).unwrap_or_default();
    let n: i64 = body
        .parse()
        .with_context(|| format!("bad duration '{s}'"))?;
    Ok(match unit {
        'm' => chrono::Duration::minutes(n),
        'h' => chrono::Duration::hours(n),
        'd' => chrono::Duration::days(n),
        'w' => chrono::Duration::weeks(n),
        _ => bail!("duration unit must be m/h/d/w, got '{unit}'"),
    })
}

/// "--since yesterday|today|1h|2d|30m|ISO-8601" -> UTC instant.
pub fn parse_since(s: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    use chrono::{Duration, Local, Utc};
    let now = Utc::now();
    let text = s.trim().to_lowercase();
    match text.as_str() {
        "yesterday" => return Ok(now - Duration::days(1)),
        "today" => {
            let midnight = Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .and_then(|t| t.and_local_timezone(Local).earliest())
                .context("cannot compute local midnight")?;
            return Ok(midnight.with_timezone(&Utc));
        }
        _ => {}
    }
    if let Ok(d) = parse_duration(&text) {
        return Ok(now - d);
    }
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(ts.with_timezone(&Utc));
    }
    bail!("cannot understand --since '{s}'; try 'yesterday', '6h', '2d', or RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_normalize() {
        assert_eq!(to_cron("every morning").unwrap(), "30 7 * * 1-5");
        assert_eq!(to_cron("morning").unwrap(), "30 7 * * 1-5");
        assert_eq!(to_cron("every 15m").unwrap(), "*/15 * * * *");
        assert_eq!(to_cron("every 5h").unwrap(), "0 */5 * * *");
        assert_eq!(to_cron("weekdays 7:30").unwrap(), "30 7 * * 1-5");
        assert_eq!(to_cron("daily 9am").unwrap(), "0 9 * * *");
        assert_eq!(to_cron("monday 9am").unwrap(), "0 9 * * 1");
        assert_eq!(to_cron("hourly").unwrap(), "0 * * * *");
        assert_eq!(to_cron("every day at 6pm").unwrap(), "0 18 * * *");
        assert_eq!(to_cron("weekly").unwrap(), "0 9 * * 1");
        assert_eq!(to_cron("weekly monday 9am").unwrap(), "0 9 * * 1");
        assert_eq!(to_cron("every weekly friday 5pm").unwrap(), "0 17 * * 5");
        assert_eq!(to_cron("each morning").unwrap(), "30 7 * * 1-5");
        assert_eq!(to_cron("30 7 * * 1-5").unwrap(), "30 7 * * 1-5");
    }

    #[test]
    fn bad_cadence_is_an_error() {
        assert!(to_cron("whenever").is_err());
        assert!(to_cron("every 90m").is_err());
    }
}
