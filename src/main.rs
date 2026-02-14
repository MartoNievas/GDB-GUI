use eframe::egui;
use std::{env, fs, io};

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions::default();

    // Obtener argumento (archivo)
    let filepath = env::args().nth(1);

    eframe::run_native(
        "egui Demo",
        options,
        Box::new(move |_cc| {
            Box::new(MyApp::new(filepath.clone()).unwrap_or_else(|e| MyApp {
                file_content: format!("Error loading file: {}", e),
            }))
        }),
    )
}

#[derive(Default)]
struct MyApp {
    file_content: String,
}

impl MyApp {
    fn new(filepath: Option<String>) -> Result<Self, io::Error> {
        let file_content = if let Some(path) = filepath {
            fs::read_to_string(path).unwrap_or_else(|_| "Failed to load file".to_string())
        } else {
            "No file provided".to_string()
        };

        Ok(Self { file_content })
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {}
}
