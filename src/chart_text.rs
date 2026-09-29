use std::{
    cell::{OnceCell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use fontdue::{Font, FontSettings, Metrics};
use tiny_skia::{Pixmap, Point, PremultipliedColorU8};

pub struct ChartFont {
    font: OnceCell<Font>,
    glyphs: RefCell<HashMap<(char, u32), Rc<ChartGlyph>>>,
}

struct ChartGlyph {
    metrics: Metrics,
    coverage: Vec<u8>,
}

#[derive(Clone, Copy)]
pub struct TextRun<'a> {
    pub text: &'a str,
    pub pixels: f32,
}

#[derive(Clone, Copy)]
pub enum TextDirection {
    Horizontal,
    Upward,
    Downward,
}

pub struct ChartLabel<'a> {
    pub run: TextRun<'a>,
    pub center: Point,
    pub direction: TextDirection,
    pub rgba: [u8; 4],
}

impl ChartFont {
    pub fn create() -> Self {
        Self {
            font: OnceCell::new(),
            glyphs: RefCell::new(HashMap::new()),
        }
    }

    fn load(&self) -> &Font {
        self.font.get_or_init(|| {
            Font::from_bytes(
                include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf") as &[u8],
                FontSettings {
                    collection_index: 0,
                    scale: 40.0,
                    load_substitutions: false,
                },
            )
            .expect("bundled JetBrains Mono must be valid")
        })
    }

    pub fn measure_width(&self, run: TextRun<'_>) -> f32 {
        let font = self.load();
        let mut previous = None;
        let mut width = 0.0;
        for character in run.text.chars() {
            width += previous
                .and_then(|left| font.horizontal_kern(left, character, run.pixels))
                .unwrap_or(0.0);
            width += font.metrics(character, run.pixels).advance_width;
            previous = Some(character);
        }
        width
    }

    pub fn measure_height(&self, pixels: f32) -> f32 {
        let metrics = self
            .load()
            .horizontal_line_metrics(pixels)
            .expect("bundled font has horizontal metrics");
        metrics.ascent - metrics.descent
    }

    fn load_glyph(&self, character: char, pixels: f32) -> Rc<ChartGlyph> {
        let key = (character, pixels.to_bits());
        if let Some(glyph) = self.glyphs.borrow().get(&key) {
            return Rc::clone(glyph);
        }
        let (metrics, coverage) = self.load().rasterize(character, pixels);
        let glyph = Rc::new(ChartGlyph { metrics, coverage });
        let mut cache = self.glyphs.borrow_mut();
        if cache.len() >= 2048 {
            cache.clear();
        }
        cache.insert(key, Rc::clone(&glyph));
        glyph
    }

    pub fn paint(&self, canvas: &mut Pixmap, label: ChartLabel<'_>) {
        let font = self.load();
        let metrics = font
            .horizontal_line_metrics(label.run.pixels)
            .expect("bundled font has horizontal metrics");
        let mut cursor = -self.measure_width(label.run) / 2.0;
        let baseline = (metrics.ascent + metrics.descent) / 2.0;
        let mut previous = None;
        for character in label.run.text.chars() {
            cursor += previous
                .and_then(|left| font.horizontal_kern(left, character, label.run.pixels))
                .unwrap_or(0.0);
            let glyph = self.load_glyph(character, label.run.pixels);
            for row in 0..glyph.metrics.height {
                for column in 0..glyph.metrics.width {
                    let alpha = (u32::from(glyph.coverage[row * glyph.metrics.width + column])
                        * u32::from(label.rgba[3])
                        + 127)
                        / 255;
                    if alpha == 0 {
                        continue;
                    }
                    let local_x = cursor + glyph.metrics.xmin as f32 + column as f32;
                    let local_y =
                        baseline - glyph.metrics.ymin as f32 - glyph.metrics.height as f32
                            + row as f32;
                    let (dx, dy) = match label.direction {
                        TextDirection::Horizontal => (local_x, local_y),
                        TextDirection::Upward => (local_y, -local_x),
                        TextDirection::Downward => (-local_y, local_x),
                    };
                    let x = (label.center.x + dx).round() as i32;
                    let y = (label.center.y + dy).round() as i32;
                    if x < 0 || y < 0 || x >= canvas.width() as i32 || y >= canvas.height() as i32 {
                        continue;
                    }
                    let offset = y as usize * canvas.width() as usize + x as usize;
                    let destination = &mut canvas.pixels_mut()[offset];
                    let blend = |source: u8, target: u8| {
                        ((u32::from(source) * alpha + u32::from(target) * (255 - alpha) + 127)
                            / 255) as u8
                    };
                    *destination = PremultipliedColorU8::from_rgba(
                        blend(label.rgba[0], destination.red()),
                        blend(label.rgba[1], destination.green()),
                        blend(label.rgba[2], destination.blue()),
                        blend(255, destination.alpha()),
                    )
                    .expect("source-over blending preserves premultiplied channels");
                }
            }
            cursor += glyph.metrics.advance_width;
            previous = Some(character);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_font_is_lazy_cached_and_renders_all_label_directions() {
        let font = ChartFont::create();
        assert!(font.font.get().is_none());
        for direction in [
            TextDirection::Horizontal,
            TextDirection::Upward,
            TextDirection::Downward,
        ] {
            let mut pixmap = Pixmap::new(180, 180).unwrap();
            for _ in 0..2 {
                font.paint(
                    &mut pixmap,
                    ChartLabel {
                        run: TextRun {
                            text: "Words per Minute 0123456789",
                            pixels: 10.0,
                        },
                        center: Point::from_xy(90.0, 90.0),
                        direction,
                        rgba: [170, 180, 200, 255],
                    },
                );
            }
            assert!(
                pixmap
                    .pixels()
                    .iter()
                    .filter(|pixel| pixel.alpha() > 0)
                    .count()
                    > 100
            );
            assert!(
                pixmap
                    .pixels()
                    .iter()
                    .any(|pixel| pixel.alpha() > 0 && pixel.alpha() < 255)
            );
            assert!(font.glyphs.borrow().len() < 30);
        }
    }
}
