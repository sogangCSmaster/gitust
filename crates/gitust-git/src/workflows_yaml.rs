//! Safe subset YAML parser (rejecting anchors, aliases, tags, merge keys, and multi-document streams).

use serde_json::{Map, Value};

use crate::GitEngineError;

use super::{invalid, MAX_WORKFLOW_BYTES};

#[derive(Clone, Debug)]
struct YamlLine {
    indent: usize,
    text: String,
}

/// Parses a safe, bounded subset of YAML 1.2 into `serde_json::Value`, failing closed on
/// anchors (`&`), aliases (`*`), merge keys (`<<:`), tags (`!`), and multi-document markers (`---`).
pub fn parse_safe_workflow_yaml(input: &str) -> Result<Value, GitEngineError> {
    if input.len() as u64 > MAX_WORKFLOW_BYTES {
        return Err(invalid("YAML workflow exceeds 64 KiB limit"));
    }
    let raw_lines: Vec<&str> = input.lines().collect();
    let mut lines = Vec::new();
    let mut idx = 0usize;
    let mut seen_content = false;

    while idx < raw_lines.len() {
        let raw = raw_lines[idx].trim_end_matches('\r');
        if raw.bytes().any(|b| b == b'\t' || b == 0) {
            return Err(invalid("YAML workflow must not contain tabs or NUL bytes"));
        }
        let indent = raw.bytes().take_while(|b| *b == b' ').count();
        let trimmed = raw[indent..].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            idx += 1;
            continue;
        }
        if trimmed == "---" || trimmed == "..." {
            if seen_content {
                return Err(invalid("multi-document YAML streams are not allowed"));
            }
            idx += 1;
            continue;
        }
        seen_content = true;
        let clean = strip_yaml_comment(trimmed);
        reject_unsafe_yaml_tokens(clean)?;

        if let Some((key_part, after_colon)) = split_yaml_key_value(clean) {
            let rhs = after_colon.trim();
            if rhs == "|" || rhs == "|-" || rhs == ">" || rhs == ">-" {
                let join_sep = if rhs.starts_with('|') { "\n" } else { " " };
                idx += 1;
                let mut block_lines = Vec::new();
                let mut block_indent: Option<usize> = None;
                while idx < raw_lines.len() {
                    let b_raw = raw_lines[idx].trim_end_matches('\r');
                    let b_ind = b_raw.bytes().take_while(|b| *b == b' ').count();
                    let b_trim = b_raw[b_ind..].trim();
                    if b_trim.is_empty() {
                        block_lines.push(String::new());
                        idx += 1;
                        continue;
                    }
                    if b_ind <= indent {
                        break;
                    }
                    let base = *block_indent.get_or_insert(b_ind);
                    let slice_start = b_ind.min(base);
                    block_lines.push(b_raw[slice_start..].to_owned());
                    idx += 1;
                }
                while block_lines.last().is_some_and(String::is_empty) {
                    block_lines.pop();
                }
                let joined = block_lines.join(join_sep);
                let encoded = serde_json::to_string(&joined).map_err(|_| invalid("YAML block"))?;
                lines.push(YamlLine {
                    indent,
                    text: format!("{key_part}: {encoded}"),
                });
                continue;
            }
        }
        lines.push(YamlLine {
            indent,
            text: clean.to_owned(),
        });
        idx += 1;
    }

    if lines.is_empty() {
        return Err(invalid("YAML workflow is empty"));
    }
    let mut cursor = 0usize;
    let val = parse_yaml_node(&lines, &mut cursor, lines[0].indent, 0)?;
    if cursor != lines.len() {
        return Err(invalid("unexpected trailing content in YAML workflow"));
    }
    Ok(val)
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_double => {
                i += 2;
                continue;
            }
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return line[..i].trim_end();
            }
            _ => {}
        }
        i += 1;
    }
    line.trim_end()
}

fn reject_unsafe_yaml_tokens(line: &str) -> Result<(), GitEngineError> {
    if line.starts_with("<<:") || line.contains(" <<:") {
        return Err(invalid("YAML merge keys ('<<:') are not allowed"));
    }
    let check_value_token = |val: &str| -> Result<(), GitEngineError> {
        let v = val.trim();
        if v.starts_with('&') || v.starts_with('*') || v.starts_with('!') {
            return Err(invalid(
                "YAML anchors ('&'), aliases ('*'), and tags ('!') are not allowed",
            ));
        }
        for tok in v.split_whitespace() {
            if (tok.starts_with('&') || tok.starts_with('*') || tok.starts_with('!'))
                && tok.len() > 1
                && tok.as_bytes()[1].is_ascii_alphanumeric()
            {
                return Err(invalid(
                    "YAML anchors ('&'), aliases ('*'), and tags ('!') are not allowed",
                ));
            }
        }
        Ok(())
    };
    if let Some((_, rhs)) = split_yaml_key_value(line) {
        check_value_token(rhs)?;
    } else if let Some(item) = line.strip_prefix("- ") {
        check_value_token(item)?;
    } else {
        check_value_token(line)?;
    }
    Ok(())
}

fn split_yaml_key_value(s: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b':' if !in_single && !in_double => {
                if i + 1 == bytes.len() || bytes[i + 1] == b' ' {
                    let key = s[..i].trim();
                    if !key.is_empty() && !key.starts_with('-') {
                        return Some((key, &s[i + 1..]));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_yaml_node(
    lines: &[YamlLine],
    cursor: &mut usize,
    base_indent: usize,
    depth: usize,
) -> Result<Value, GitEngineError> {
    if depth > 12 || *cursor >= lines.len() {
        return Err(invalid("YAML nesting depth exceeded or unexpected EOF"));
    }
    if lines[*cursor].text == "-" || lines[*cursor].text.starts_with("- ") {
        parse_yaml_seq(lines, cursor, base_indent, depth)
    } else {
        parse_yaml_map(lines, cursor, base_indent, depth)
    }
}

fn parse_yaml_seq(
    lines: &[YamlLine],
    cursor: &mut usize,
    base_indent: usize,
    depth: usize,
) -> Result<Value, GitEngineError> {
    let mut arr = Vec::new();
    while *cursor < lines.len() {
        let line = &lines[*cursor];
        if line.indent < base_indent {
            break;
        }
        if line.indent != base_indent {
            return Err(invalid("invalid YAML sequence indentation"));
        }
        let rest = if line.text == "-" {
            ""
        } else if let Some(r) = line.text.strip_prefix("- ") {
            r.trim()
        } else {
            break;
        };
        *cursor += 1;
        if rest.is_empty() {
            if *cursor < lines.len() && lines[*cursor].indent > base_indent {
                let child_indent = lines[*cursor].indent;
                arr.push(parse_yaml_node(lines, cursor, child_indent, depth + 1)?);
            } else {
                arr.push(Value::Null);
            }
        } else if let Some((k, v)) = split_yaml_key_value(rest) {
            let mut map = Map::new();
            let clean_k = unquote_yaml_str(k)?;
            let v_trim = v.trim();
            let val = if v_trim.is_empty() {
                if *cursor < lines.len() && lines[*cursor].indent > base_indent {
                    let child_indent = lines[*cursor].indent;
                    parse_yaml_node(lines, cursor, child_indent, depth + 1)?
                } else {
                    Value::Null
                }
            } else {
                parse_yaml_scalar_or_inline(v_trim)?
            };
            map.insert(clean_k, val);
            if *cursor < lines.len() && lines[*cursor].indent > base_indent {
                let extra_indent = lines[*cursor].indent;
                if let Value::Object(extra_map) =
                    parse_yaml_map(lines, cursor, extra_indent, depth + 1)?
                {
                    for (ek, ev) in extra_map {
                        map.insert(ek, ev);
                    }
                }
            }
            arr.push(Value::Object(map));
        } else {
            arr.push(parse_yaml_scalar_or_inline(rest)?);
        }
    }
    Ok(Value::Array(arr))
}

fn parse_yaml_map(
    lines: &[YamlLine],
    cursor: &mut usize,
    base_indent: usize,
    depth: usize,
) -> Result<Value, GitEngineError> {
    let mut map = Map::new();
    while *cursor < lines.len() {
        let line = &lines[*cursor];
        if line.indent < base_indent {
            break;
        }
        if line.indent != base_indent {
            return Err(invalid("invalid YAML mapping indentation"));
        }
        if line.text == "-" || line.text.starts_with("- ") {
            break;
        }
        let (k, v) = split_yaml_key_value(&line.text)
            .ok_or_else(|| invalid(format!("expected YAML 'key: value' at '{}'", line.text)))?;
        let clean_k = unquote_yaml_str(k)?;
        let v_trim = v.trim();
        *cursor += 1;
        let val = if v_trim.is_empty() {
            if *cursor < lines.len() && lines[*cursor].indent > base_indent {
                let child_indent = lines[*cursor].indent;
                parse_yaml_node(lines, cursor, child_indent, depth + 1)?
            } else {
                Value::Null
            }
        } else {
            parse_yaml_scalar_or_inline(v_trim)?
        };
        if map.insert(clean_k, val).is_some() {
            return Err(invalid("duplicate key in YAML mapping"));
        }
    }
    Ok(Value::Object(map))
}

fn parse_yaml_scalar_or_inline(raw: &str) -> Result<Value, GitEngineError> {
    let s = raw.trim();
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        let mut items = Vec::new();
        if !inner.trim().is_empty() {
            for part in inner.split(',') {
                items.push(parse_yaml_scalar_or_inline(part.trim())?);
            }
        }
        return Ok(Value::Array(items));
    }
    if s == "true" {
        return Ok(Value::Bool(true));
    }
    if s == "false" {
        return Ok(Value::Bool(false));
    }
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        let n = s
            .parse::<u64>()
            .map_err(|_| invalid("invalid integer in YAML"))?;
        return Ok(Value::Number(n.into()));
    }
    Ok(Value::String(unquote_yaml_str(s)?))
}

fn unquote_yaml_str(s: &str) -> Result<String, GitEngineError> {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        serde_json::from_str::<String>(t).map_err(|_| invalid("invalid double-quoted YAML string"))
    } else if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        Ok(t[1..t.len() - 1].replace("''", "'"))
    } else {
        Ok(t.to_owned())
    }
}
