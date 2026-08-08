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

/// True for Source water/refract shaders, following patch includes.
pub fn is_water_or_refract(get: &impl Fn(&str) -> Option<Vec<u8>>, material_name: &str) -> bool {
    let mat = normalize_path(material_name);
    let mut vmt_path = format!("materials/{mat}.vmt");
    let mut depth = 0;
    while depth < 4 {
        depth += 1;
        let Some(text) = get(&vmt_path).and_then(|b| decode_text(&b)) else {
            return false;
        };
        let shader = first_token(&text).to_ascii_lowercase();
        if shader == "water" || shader == "refract" {
            return true;
        }
        let Some(include) = find_quoted_value(&text, "include") else {
            return false;
        };
        vmt_path = normalize_path(&include);
        if !vmt_path.ends_with(".vmt") {
            vmt_path = format!("materials/{}.vmt", strip_materials_prefix(&vmt_path));
        }
    }
    false
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

fn first_token(text: &str) -> &str {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("");
    }
    text.split_whitespace().next().unwrap_or("")
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
        if let Some(&next) = text.as_bytes().get(after) {
            if next.is_ascii_alphanumeric() || next == b'_' || next == b'$' || next == b'%' {
                search_from = after;
                continue;
            }
        }
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
    fn basetexture_does_not_match_basetexture2() {
        let t = r#"WorldVertexTransition {
            $basetexture2 "elly/nature_moss_2k_albedo"
            $basetexture "elly/nature_moss3_2k_albedo"
        }"#;
        assert_eq!(
            find_basetexture(t).as_deref(),
            Some("elly/nature_moss3_2k_albedo")
        );
    }

    #[test]
    fn parses_include() {
        let t = r#""patch" { "include" "materials/CASTLE/FLOOR03.vmt" }"#;
        assert_eq!(
            find_quoted_value(t, "include").as_deref(),
            Some("materials/CASTLE/FLOOR03.vmt")
        );
    }

    #[test]
    fn recognizes_included_water_shader() {
        let get = |path: &str| match path {
            "materials/maps/test/water_depth.vmt" => {
                Some(br#""patch" { "include" "materials/nature/water.vmt" }"#.to_vec())
            }
            "materials/nature/water.vmt" => Some(br#""Water" { "$normalmap" "water/n" }"#.to_vec()),
            _ => None,
        };
        assert!(is_water_or_refract(&get, "maps/test/water_depth"));
    }
}
