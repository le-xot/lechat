use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrcLine {
    pub tags: HashMap<String, String>,
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

pub fn parse_line(raw: &str) -> Option<IrcLine> {
    let raw = raw.trim_end_matches(['\r', '\n']);
    if raw.is_empty() {
        return None;
    }

    let mut rest = raw;

    let mut tags = HashMap::new();
    if let Some(stripped) = rest.strip_prefix('@') {
        let end = stripped.find(' ')?;
        for pair in stripped[..end].split(';') {
            if let Some((key, value)) = pair.split_once('=') {
                tags.insert(key.to_string(), unescape_tag(value));
            }
        }
        rest = stripped[end..].trim_start();
    }

    let mut prefix = None;
    if let Some(stripped) = rest.strip_prefix(':') {
        let end = stripped.find(' ')?;
        prefix = Some(stripped[..end].to_string());
        rest = stripped[end..].trim_start();
    }

    let (command, tail) = match rest.find(' ') {
        Some(pos) => (&rest[..pos], rest[pos + 1..].trim_start()),
        None => (rest, ""),
    };

    let mut params = Vec::new();
    if !tail.is_empty() {
        let trailing = tail.find(" :").map(|pos| pos + 1);
        match trailing {
            Some(pos) => {
                let head = tail[..pos].trim();
                for param in head.split_whitespace() {
                    params.push(param.to_string());
                }
                params.push(tail[pos + 1..].to_string());
            }
            None => {
                for param in tail.split_whitespace() {
                    params.push(param.to_string());
                }
            }
        }
    }

    Some(IrcLine {
        tags,
        prefix,
        command: command.to_string(),
        params,
    })
}

pub fn unescape_tag(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some(':') => out.push(';'),
            Some('\\') => out.push('\\'),
            Some('r') | Some('n') | None => {}
            Some(other) => out.push(other),
        }
    }
    out
}

pub fn is_ping(line: &IrcLine) -> bool {
    line.command.eq_ignore_ascii_case("PING")
}

pub fn parse_room_id(line: &IrcLine) -> Option<String> {
    if !line.command.eq_ignore_ascii_case("ROOMSTATE") {
        return None;
    }
    line.tags.get("room-id").cloned()
}

/// Parses the `emotes` tag: `id:start-end,start-end/id:start-end` into
/// character ranges (inclusive).
pub fn parse_emotes_tag(value: &str) -> Vec<(String, Vec<(usize, usize)>)> {
    let mut result = Vec::new();
    if value.trim().is_empty() {
        return result;
    }
    for group in value.split('/') {
        let Some((id, ranges)) = group.split_once(':') else {
            continue;
        };
        let mut parsed = Vec::new();
        for range in ranges.split(',') {
            let Some((start, end)) = range.split_once('-') else {
                continue;
            };
            if let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) {
                parsed.push((start, end));
            }
        }
        if !parsed.is_empty() {
            result.push((id.to_string(), parsed));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_privmsg_tags() {
        let raw = r#"@badge-info=subscriber/69;badges=subscriber/66,premium/1;color=#DAA520;display-name=leman0va;emotes=;id=7cdb25d2;tmi-sent-ts=1793656722692;user-id=427746412 :leman0va!leman0va@leman0va.tmi.twitch.tv PRIVMSG #xqc :LOL i can't"#;
        let line = parse_line(raw).unwrap();
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.params[0], "#xqc");
        assert_eq!(line.params[1], "LOL i can't");
        assert_eq!(line.tags.get("display-name").unwrap(), "leman0va");
        assert_eq!(line.tags.get("color").unwrap(), "#DAA520");
    }

    #[test]
    fn parses_ping_and_room_state() {
        assert!(is_ping(&parse_line("PING :tmi.twitch.tv").unwrap()));
        let line = parse_line(
            "@emote-only=0;room-id=1263747798;slow=0 :tmi.twitch.tv ROOMSTATE #etonelexot",
        )
        .unwrap();
        assert_eq!(parse_room_id(&line).as_deref(), Some("1263747798"));
    }

    #[test]
    fn parses_emote_ranges_in_chars() {
        let ranges = parse_emotes_tag("25:0-4,12-16/1902:18-22");
        assert_eq!(
            ranges,
            vec![
                ("25".to_string(), vec![(0, 4), (12, 16)]),
                ("1902".to_string(), vec![(18, 22)]),
            ]
        );
    }

    #[test]
    fn unescapes_tag_values() {
        assert_eq!(unescape_tag(r"a\sb\:c\\d"), "a b;c\\d");
    }
}
