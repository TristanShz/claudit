//! Generates the list of SQL migrations from `migrations/*.sql`.
//!
//! Each ticket adds its own migration file (named `NNNN_description.sql`,
//! where `NNNN` is the ticket number), so parallel work never edits a shared
//! list. Files are applied in lexical order of their names.

use std::{env, fs, path::Path};

fn main() {
    let dir = Path::new("migrations");
    println!("cargo:rerun-if-changed=migrations");

    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("migrations/ directory must exist")
        .map(|entry| entry.expect("readable migrations entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".sql"))
        .collect();
    names.sort();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let mut out = String::from("pub(crate) const MIGRATIONS: &[(&str, &str)] = &[\n");
    for name in &names {
        println!("cargo:rerun-if-changed=migrations/{name}");
        let id = name.trim_end_matches(".sql");
        out.push_str(&format!(
            "    ({id:?}, include_str!({path:?})),\n",
            path = format!("{manifest_dir}/migrations/{name}")
        ));
    }
    out.push_str("];\n");

    let out_path = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("migrations.rs");
    fs::write(out_path, out).expect("write generated migrations list");
}
