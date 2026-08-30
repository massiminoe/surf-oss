//! Dump raw entity key/values matching a class or name substring.
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: ent_dump <bsp> [filter]");
    let filter = args.next().unwrap_or_default().to_ascii_lowercase();
    let data = std::fs::read(&path).expect("read");
    let bsp = vbsp::Bsp::read(&data).expect("bsp");
    for ent in bsp.entities.iter() {
        let raw: Vec<(String, String)> = ent
            .properties()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let joined = raw
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("  ");
        if filter.is_empty() || joined.to_ascii_lowercase().contains(&filter) {
            println!("{joined}");
        }
    }
}
