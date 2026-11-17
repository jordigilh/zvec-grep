use std::io::Write;
use zg_code_ir::{InputFile, extract};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = "// é 🚀\r\nfn add() { save(); }\r\n";
    let (snapshot, _) = extract(
        "demo",
        &[InputFile {
            root_id: "main",
            relative_path: "src/add.rs",
            language: "rust",
            bytes: source.as_bytes(),
        }],
    )?;
    writeln!(
        std::io::stdout().lock(),
        "{}",
        serde_json::json!({"source": source, "snapshot": snapshot})
    )?;
    Ok(())
}
