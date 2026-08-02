//! Minimal VMT parse: `$basetexture` + `patch`/`include`.

use crate::pak::normalize_path;

/// Resolve a material name (e.g. `CASTLE/STEP01`) to a basetexture path
/// (e.g. `castle/step01`) via a byte getter (`materials/<name>.vmt`).
pub fn resolve_basetexture(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
) -> Option<String> {
    let mat = normalize_path(material_name);
    let mut vmt_path = format!("materials/{mat}.vmt");
    let mut depth = 0;
    while depth < 4 {
        depth += 1;
        let text = get(&vmt_path).and_then(|b| decode_text(&b))?;
        if let Some(include) = find_quoted_value(&text, "include") {
            vmt_path = normalize_path(&include);
            if !vmt_path.ends_with(".vmt") {
                vmt_path = format!("materials/{}.vmt", strip_materials_prefix(&vmt_path));
            }
            if let Some(bt) = find_basetexture(&text) {
                return Some(normalize_path(&bt));
            }
            continue;
        }
        if let Some(bt) = find_basetexture(&text) {
            return Some(normalize_path(&bt));
        }
        return None;
    }
    None
}

fn strip_materials_prefix(path: &str) -> &str {
    let p = path.strip_prefix("materials/").unwrap_or(path);
    p.strip_suffix(".vmt").unwrap_or(p)
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec())
        .ok()
        .or_else(|| Some(bytes.iter().map(|&b| b as char).collect()))
}

fn find_basetexture(text: &str) -> Option<String> {
    find_key_value(text, "$basetexture")
}

fn find_key_value(text: &str, key: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let key_l = key.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find(&key_l) {
        let at = search_from + rel;
        if at > 0 {
            let prev = text.as_bytes()[at - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'$' || prev == b'%' {
                search_from = at + key_l.len();
                continue;
            }
        }
        let after = at + key_l.len();
        let mut rest = &text[after..];
        if rest.starts_with('"') {
            rest = &rest[1..];
        }
        if let Some(val) = parse_value_after(rest) {
            return Some(val);
        }
        search_from = after;
    }
    None
}

fn find_quoted_value(text: &str, key: &str) -> Option<String> {
    find_key_value(text, key)
}

fn parse_value_after(s: &str) -> Option<String> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    if s.starts_with('"') {
        let rest = &s[1..];
        let end = rest.find('"')?;
        return Some(rest[..end].trim().to_string());
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() || c == '{' || c == '}' {
            break;
        }
        end = i + c.len_utf8();
    }
    if end == 0 {
        return None;
    }
    Some(s[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_basetexture() {
        let t = r#"LightmappedGeneric { $basetexture "castle/step01" }"#;
        assert_eq!(find_basetexture(t).as_deref(), Some("castle/step01"));
    }

    #[test]
    fn parses_include() {
        let t = r#""patch" { "include" "materials/CASTLE/FLOOR03.vmt" }"#;
        assert_eq!(
            find_quoted_value(t, "include").as_deref(),
            Some("materials/CASTLE/FLOOR03.vmt")
        );
    }
}
