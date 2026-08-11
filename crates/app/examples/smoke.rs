//! The smallest possible eframe application, used as a control when
//! diagnosing "the window opens but nothing is drawn".
//!
//! bettershot's editor and this example share the same windowing stack, so
//! running both in the same session separates a bug in bettershot from a
//! limitation of the session. If this example does not reach its first frame
//! either, the problem is below bettershot.
//!
//! ```sh
//! cargo run -p bettershot --example smoke
//! ```
//!
//! It prints `smoke: first frame after Nms` and exits.

use std::time::Instant;

struct Smoke {
    started: Instant,
    frames: u32,
}

impl eframe::App for Smoke {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames += 1;
        if self.frames == 1 {
            println!(
                "smoke: first frame after {:.1}ms",
                self.started.elapsed().as_secs_f64() * 1000.0
            );
        }
        ui.label("bettershot smoke test");
        // Exit once a few frames have been drawn, so this can be run
        // unattended.
        if self.frames >= 3 {
            println!("smoke: rendered {} frames, exiting", self.frames);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    println!("smoke: starting");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("bettershot smoke")
            .with_inner_size([320.0, 240.0]),
        ..Default::default()
    };

    eframe::run_native(
        "bettershot-smoke",
        options,
        Box::new(move |_cc| Ok(Box::new(Smoke { started, frames: 0 }))),
    )?;

    println!("smoke: event loop returned cleanly");
    Ok(())
}
