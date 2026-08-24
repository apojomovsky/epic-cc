// Device numbers are the one input with no oracle upstream of banking and
// paging, so a TOML must say where its values came from. Included by both
// build.rs and tests/provenance.rs to keep a single definition.

fn validate_provenance(path: &str, root: &toml::Value) -> Result<(), String> {
    let p = root
        .get("provenance")
        .ok_or_else(|| format!("device: {path}: missing [provenance] table"))?;
    let field = |k: &str| p.get(k).and_then(|v| v.as_str());
    let tier = field("tier").ok_or_else(|| format!("device: {path}: [provenance] needs a tier"))?;

    let required: &[&str] = match tier {
        "atdf" => &["source", "pack", "sha256"],
        "datasheet" => &["document", "ticket"],
        other => {
            return Err(format!(
                "device: {path}: unknown provenance tier {other:?}, expected atdf or datasheet"
            ))
        }
    };
    for key in required {
        if field(key).is_none_or(str::is_empty) {
            return Err(format!(
                "device: {path}: provenance tier {tier:?} requires a non-empty {key}"
            ));
        }
    }
    Ok(())
}
