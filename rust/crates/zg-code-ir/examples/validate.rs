use std::{collections::BTreeMap, env, fs, path::Path};
use zg_code_ir::{Snapshot, validate};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err("usage: validate SNAPSHOT_JSON SOURCE_ROOT".into());
    }
    let snapshot: Snapshot = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut sources = BTreeMap::new();
    for file in &snapshot.files {
        let path = Path::new(&file.relative_path);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("invalid relative path".into());
        }
        sources.insert(
            file.file_id.clone(),
            fs::read(Path::new(&args[2]).join(path))?,
        );
    }
    validate(&snapshot, &sources).map_err(Into::into)
}
