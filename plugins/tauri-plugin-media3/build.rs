const COMMANDS: &[&str] = &[
    "media3_snapshot",
    "media3_emit",
    "media3_clamp_seek",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
