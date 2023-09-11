#[cfg(feature = "generate-bindings")]
use {std::env, std::path::PathBuf};

fn main() {
    #[cfg(feature = "generate-bindings")]
    {
        let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());

        // Not ideal to navigate by .. but there isn't a good alternative until cargo provides some
        // sort of artifacts dir, see https://github.com/rust-lang/cargo/issues/9096
        let bindings_path = out_path.join("../../../include/nic-emu.hpp");

        cbindgen::Builder::new()
            .with_crate(crate_dir)
            .with_pragma_once(true)
            .with_include_version(true)
            .exclude_item("DESCRIPTOR_BUFFER_SIZE")
            .generate()
            .expect("Unable to generate bindings")
            .write_to_file(bindings_path);
    }
}
