mod sys;
// Spike: prove ratatui types + a custom-buffer render compile to wasm32-wasip1
// without crossterm. Replaced by the real UI in a later task.
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Gauge, Widget};

fn main() {
    let area = Rect::new(0, 0, 40, 1);
    let mut buf = Buffer::empty(area);
    Gauge::default().percent(42).render(area, &mut buf);
    // Print the first row's symbols as proof of a rendered frame.
    let mut line = String::new();
    for x in 0..area.width {
        line.push_str(buf[(x, 0)].symbol());
    }
    println!("rtop-spike: {}", line);
}
