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

/// Which see-through Source shader a material uses, if any. We have no
/// alpha-blended pass, so each gets a different opaque stand-in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeeThrough {
    /// `Water` — a body of water. A flat opaque teal reads correctly.
    Water,
    /// `Refract` — ice, glass, crystal. Teal does *not* read correctly here:
    /// boreas' icicles came out as saturated blue slabs. Frosted white-blue is
    /// the honest stand-in, and unlike making them invisible it can never hide
    /// a surface the player is meant to see.
    Refract,
}

/// Classify a material's shader, following patch includes.
pub fn see_through_kind(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
) -> Option<SeeThrough> {
    let mat = normalize_path(material_name);
    let mut vmt_path = format!("materials/{mat}.vmt");
    let mut depth = 0;
    while depth < 4 {
        depth += 1;
        let text = get(&vmt_path).and_then(|b| decode_text(&b))?;
        match first_token(&text).to_ascii_lowercase().as_str() {
            "water" => return Some(SeeThrough::Water),
            "refract" => return Some(SeeThrough::Refract),
            _ => {}
        }
        let include = find_quoted_value(&text, "include")?;
        vmt_path = normalize_path(&include);
        if !vmt_path.ends_with(".vmt") {
            vmt_path = format!("materials/{}.vmt", strip_materials_prefix(&vmt_path));
        }
    }
    None
}

/// True when the material declares `$alphatest` or `$translucent` (following
/// `patch`/`include` chains).
///
/// Only these keep their cutout alpha. Source reuses the alpha channel of
/// ordinary textures for envmap masks and `$selfillum`, so alpha-testing
/// unconditionally would punch holes in — or erase entirely — perfectly opaque
/// world surfaces. Foliage cards (boreas' pines) are the case this exists for:
/// without it every branch card draws as an opaque black quad.
pub fn is_alpha_tested(get: &impl Fn(&str) -> Option<Vec<u8>>, material_name: &str) -> bool {
    let mat = normalize_path(material_name);
    let mut vmt_path = format!("materials/{mat}.vmt");
    let mut depth = 0;
    while depth < 4 {
        depth += 1;
        let Some(text) = get(&vmt_path).and_then(|b| decode_text(&b)) else {
            return false;
        };
        for key in ["$alphatest", "$translucent"] {
            if let Some(v) = find_key_value(&text, key) {
                if v.trim() != "0" {
                    return true;
                }
            }
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
    fn alphatest_is_read_through_a_patch_include() {
        let get = |path: &str| match path {
            "materials/foliage/leaf_patch.vmt" => {
                Some(br#""patch" { "include" "materials/foliage/leaf.vmt" }"#.to_vec())
            }
            "materials/foliage/leaf.vmt" => Some(
                br#""VertexLitGeneric" { "$basetexture" "foliage/leaf" "$alphatest" "1" }"#
                    .to_vec(),
            ),
            // Opaque rock that happens to carry an envmap mask in alpha.
            "materials/rock/rock01.vmt" => Some(
                br#""LightmappedGeneric" { "$basetexture" "rock/r" "$basealphaenvmapmask" "1" }"#
                    .to_vec(),
            ),
            _ => None,
        };
        assert!(is_alpha_tested(&get, "foliage/leaf_patch"));
        assert!(!is_alpha_tested(&get, "rock/rock01"));
    }

    #[test]
    fn alphatest_zero_is_not_alpha_tested() {
        let get = |path: &str| match path {
            "materials/x/y.vmt" => Some(br#""VertexLitGeneric" { "$alphatest" "0" }"#.to_vec()),
            _ => None,
        };
        assert!(!is_alpha_tested(&get, "x/y"));
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
        assert_eq!(
            see_through_kind(&get, "maps/test/water_depth"),
            Some(SeeThrough::Water)
        );
    }

    #[test]
    fn refract_is_distinguished_from_water() {
        let get = |path: &str| match path {
            "materials/ice/icicle.vmt" => Some(br#""Refract" { "$normalmap" "ice/n" }"#.to_vec()),
            _ => None,
        };
        assert_eq!(
            see_through_kind(&get, "ice/icicle"),
            Some(SeeThrough::Refract)
        );
    }
}
