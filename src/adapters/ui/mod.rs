pub mod banner;
pub mod progress;
pub mod tui;

/// Prints the welcome banner and applies the neon theme for all subsequent inquire prompts.
/// Call once at startup (e.g. in main after tracing init).
pub fn init_ui() {
    banner::print_welcome();
    tui::apply_theme();
}
