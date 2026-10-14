fn main() {
    let src_dir = std::path::Path::new("vendor/tree-sitter-pkl/src");

    let mut c_config = cc::Build::new();
    c_config.std("c11").include(src_dir);

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        c_config.flag("-utf-8");
    }

    for source in ["parser.c", "scanner.c"] {
        let path = src_dir.join(source);
        c_config.file(&path);
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        src_dir.join("tree_sitter/parser.h").display()
    );

    c_config.compile("tree-sitter-pkl");
}
