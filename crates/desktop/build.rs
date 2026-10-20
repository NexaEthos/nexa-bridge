fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-env-changed=NEXA_AGORA_URL");
    nexa_build_info::emit()?;
    tauri_build::build();
    Ok(())
}
