fn main() {
    #[cfg(windows)]
    {
        cc::Build::new()
            .file("src/engine/asio_seh.c")
            .compile("hifi_asio_seh");
    }
    tauri_build::build()
}
