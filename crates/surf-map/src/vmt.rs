//! Minimal VMT parse: `$basetexture` + `patch`/`include`.

use crate::pak::normalize_path;

/// Resolve a material name (e.g. `CASTLE/STEP01`) to a basetexture path
/// (e.g. `castle/step01`) via a byte getter (`materials/<name>.vmt`).
pub fn resolve_basetexture(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
) -> Option<String> {
    resolve_texture_key(get, material_name, "$basetexture")
}

/// `$basetexture2` — the second layer of a `WorldVertexTransition`
/// displacement material, blended in by per-vertex alpha. `None` for every
/// single-texture material.
pub fn resolve_basetexture2(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
) -> Option<String> {
    resolve_texture_key(get, material_name, "$basetexture2")
}

fn resolve_texture_key(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
    key: &str,
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
            if let Some(bt) = find_key_value(&text, key) {
                return Some(normalize_path(&bt));
            }
            continue;
        }
        if let Some(bt) = find_key_value(&text, key) {
            return Some(normalize_path(&bt));
        }
        return None;
    }
    None
}

/// Which see-through Source shader a material uses, if any. Neither is
/// rendered — no refraction, no water surface — so each gets a different
/// opaque stand-in. (`$translucent` and `$additive` *are* blended now; these
/// two are whole shaders, not blend modes.)
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
    flag_through_includes(get, material_name, &["$alphatest", "$translucent"])
}

/// True when the material declares `$translucent` (following `patch`/`include`).
///
/// `$translucent` and `$alphatest` are not the same instruction. A cutout keeps
/// or drops a texel; a translucent surface *blends* with what is behind it.
/// Testing a translucent gradient at 0.5 turns it into a hard-edged opaque
/// slab — which is what aquaflow's ocean-wall fades and tunnel glass became.
pub fn is_translucent(get: &impl Fn(&str) -> Option<Vec<u8>>, material_name: &str) -> bool {
    flag_through_includes(get, material_name, &["$translucent"])
}

/// True when the material is drawn `$additive` (following `patch`/`include`).
///
/// Additive materials *add* light: a black texel contributes nothing at all.
/// Drawn opaque — which is all we could do before the blended pass — cyberwave's
/// energy ball became a giant black disc over the skyline, the single most
/// "broken texture" looking thing on the map.
pub fn is_additive(get: &impl Fn(&str) -> Option<Vec<u8>>, material_name: &str) -> bool {
    flag_through_includes(get, material_name, &["$additive"])
}

/// True when any of `keys` is present and non-zero, following patch includes.
fn flag_through_includes(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
    keys: &[&str],
) -> bool {
    let mat = normalize_path(material_name);
    let mut vmt_path = format!("materials/{mat}.vmt");
    let mut depth = 0;
    while depth < 4 {
        depth += 1;
        let Some(text) = get(&vmt_path).and_then(|b| decode_text(&b)) else {
            return false;
        };
        for key in keys {
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

/// A material's constant colour multiplier (`$color2`, else `$color`), as a
/// **linear** RGB factor, following `patch`/`include` chains.
///
/// This is not decoration: Source ships a plain white `lights/white.vtf` and
/// maps recolour it per material, so on cyberwave all twelve neon strips are
/// the same white texture and *only* `$color2` tells them apart. Ignore it and
/// a neon map renders monochrome.
///
/// Source spells the value two ways and they are not the same number:
/// `{20 114 255}` is 0–255 in **gamma** space, `[1 .25 .5]` is already linear.
pub fn resolve_tint(
    get: &impl Fn(&str) -> Option<Vec<u8>>,
    material_name: &str,
) -> Option<[f32; 3]> {
    let mat = normalize_path(material_name);
    let mut vmt_path = format!("materials/{mat}.vmt");
    let mut depth = 0;
    while depth < 4 {
        depth += 1;
        let text = get(&vmt_path).and_then(|b| decode_text(&b))?;
        // `$color2` wins: a patch that sets both means the second to override.
        // Try every occurrence — `Proxies` blocks animate the value by writing
        // `$color2[1]`, and those component writes must not shadow the real one.
        for key in ["$color2", "$color"] {
            for v in find_key_values(&text, key) {
                if let Some(c) = parse_color(&v) {
                    return Some(c);
                }
            }
        }
        let include = find_quoted_value(&text, "include")?;
        vmt_path = normalize_path(&include);
        if !vmt_path.ends_with(".vmt") {
            vmt_path = format!("materials/{}.vmt", strip_materials_prefix(&vmt_path));
        }
    }
    None
}

/// `{r g b}` (0–255, gamma) | `[r g b]` (0–1, linear) | bare `r g b`.
fn parse_color(raw: &str) -> Option<[f32; 3]> {
    let s = raw.trim();
    let gamma = s.starts_with('{');
    let body = s.trim_matches(|c| c == '{' || c == '}' || c == '[' || c == ']');
    let mut it = body
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty());
    let mut out = [0.0f32; 3];
    for slot in out.iter_mut() {
        *slot = it.next()?.parse::<f32>().ok()?;
    }
    if it.next().is_some() {
        return None;
    }
    if gamma {
        // 0–255 gamma → linear, the way the engine reads a braced colour.
        for c in out.iter_mut() {
            *c = (*c / 255.0).clamp(0.0, 1.0).powf(2.2);
        }
    } else {
        for c in out.iter_mut() {
            *c = c.clamp(0.0, 8.0);
        }
    }
    // An all-white tint is the identity; treat it as "no tint" so it never
    // splits a texture into two identical atlas layers.
    if out.iter().all(|&c| (c - 1.0).abs() < 1e-4) {
        return None;
    }
    Some(out)
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

fn first_token(text: &str) -> &str {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("");
    }
    text.split_whitespace().next().unwrap_or("")
}

fn find_key_value(text: &str, key: &str) -> Option<String> {
    find_key_values(text, key).into_iter().next()
}

/// Every value assigned to `key`, in file order.
fn find_key_values(text: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
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
            out.push(val);
        }
        search_from = after;
    }
    out
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
        assert_eq!(find_key_value(t, "$basetexture").as_deref(), Some("castle/step01"));
    }

    #[test]
    fn basetexture_does_not_match_basetexture2() {
        let t = r#"WorldVertexTransition {
            $basetexture2 "elly/nature_moss_2k_albedo"
            $basetexture "elly/nature_moss3_2k_albedo"
        }"#;
        assert_eq!(
            find_key_value(t, "$basetexture").as_deref(),
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

    #[test]
    fn braced_color_is_gamma_0_255_and_bracketed_is_linear() {
        // cyberwave/neon_blue.vmt and models/cyberwave/neon_magenta_pulse.vmt.
        let braced = parse_color("{20 114 255}").unwrap();
        assert!((braced[2] - 1.0).abs() < 1e-4, "255 must be full scale");
        assert!(braced[0] < 0.01 && braced[1] > 0.1 && braced[1] < 0.25);
        let bracketed = parse_color("[1 0.25 0.5]").unwrap();
        assert_eq!(bracketed, [1.0, 0.25, 0.5]);
    }

    #[test]
    fn white_tint_is_no_tint() {
        assert!(parse_color("{255 255 255}").is_none());
        assert!(parse_color("[1 1 1]").is_none());
        assert!(parse_color("garbage").is_none());
        assert!(parse_color("{1 2}").is_none());
    }

    #[test]
    fn tint_is_read_through_a_patch_include_and_color2_wins() {
        let get = |path: &str| match path {
            "materials/models/cyberwave/neon_blue.vmt" => Some(
                br#""unlitgeneric" { "$basetexture" "lights/white" "$color2" "{20 114 255}" }"#
                    .to_vec(),
            ),
            "materials/patched.vmt" => Some(
                br#""patch" { "include" "materials/models/cyberwave/neon_blue.vmt" }"#.to_vec(),
            ),
            "materials/both.vmt" => {
                Some(br#""unlitgeneric" { "$color" "[1 0 0]" "$color2" "[0 1 0]" }"#.to_vec())
            }
            _ => None,
        };
        let direct = resolve_tint(&get, "models/cyberwave/neon_blue").unwrap();
        let patched = resolve_tint(&get, "patched").unwrap();
        assert_eq!(direct, patched);
        assert_eq!(resolve_tint(&get, "both"), Some([0.0, 1.0, 0.0]));
        assert_eq!(resolve_tint(&get, "nope"), None);
    }

    #[test]
    fn additive_is_read_through_a_patch_include() {
        let get = |path: &str| match path {
            "materials/effects/emp_ball1.vmt" => Some(
                br#""UnlitGeneric" { "$basetexture" "effects/emp_ball1" "$additive" "1" }"#
                    .to_vec(),
            ),
            "materials/maps/m/effects/emp_ball1_0_0_0.vmt" => {
                Some(br#""patch" { "include" "materials/effects/emp_ball1.vmt" }"#.to_vec())
            }
            "materials/plain.vmt" => Some(br#""UnlitGeneric" { "$additive" "0" }"#.to_vec()),
            _ => None,
        };
        assert!(is_additive(&get, "effects/emp_ball1"));
        assert!(is_additive(&get, "maps/m/effects/emp_ball1_0_0_0"));
        assert!(!is_additive(&get, "plain"));
        assert!(!is_additive(&get, "absent"));
    }

    #[test]
    fn translucent_is_narrower_than_alpha_tested() {
        let get = |path: &str| match path {
            "materials/glass.vmt" => {
                Some(br#""LightmappedGeneric" { "$translucent" "1" }"#.to_vec())
            }
            "materials/fence.vmt" => Some(br#""LightmappedGeneric" { "$alphatest" "1" }"#.to_vec()),
            _ => None,
        };
        // Both keep their alpha channel; only one of them blends.
        assert!(is_alpha_tested(&get, "glass") && is_translucent(&get, "glass"));
        assert!(is_alpha_tested(&get, "fence") && !is_translucent(&get, "fence"));
    }
}
