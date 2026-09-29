use std::io::{self, Write};

use crossterm::{
    cursor::{MoveTo, RestorePosition, SavePosition},
    execute, queue,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate, WindowSize},
};

use crate::chart_plot::{CellPixels, ChartImage};
use ratatui::layout::Rect;

pub struct GraphicsEnvironment<'a> {
    pub term: Option<&'a str>,
    pub program: Option<&'a str>,
    pub is_multiplexed: bool,
}

pub struct KittyGraphics {
    image_ids: [u32; 2],
    active_slot: usize,
    pending_slot: Option<usize>,
    is_placed: bool,
    last_image: Option<ChartImage>,
}

impl KittyGraphics {
    pub fn create(image_id: u32) -> Self {
        let image_id = image_id.max(1);
        Self {
            image_ids: [image_id, image_id.wrapping_add(1).max(1)],
            active_slot: 0,
            pending_slot: None,
            is_placed: false,
            last_image: None,
        }
    }

    pub fn supports_environment(input: GraphicsEnvironment<'_>) -> bool {
        if input.is_multiplexed {
            return false;
        }
        matches!(
            input.term,
            Some("xterm-kitty" | "xterm-ghostty" | "wezterm")
        ) || input.program.is_some_and(|program| {
            ["kitty", "ghostty", "wezterm"]
                .iter()
                .any(|supported| program.eq_ignore_ascii_case(supported))
        })
    }

    pub fn resolve_cell_pixels(window: WindowSize) -> Option<CellPixels> {
        if window.columns == 0 || window.rows == 0 {
            return None;
        }
        let width = window.width / window.columns;
        let height = window.height / window.rows;
        (width > 0 && height > 0).then_some(CellPixels { width, height })
    }

    pub fn update(&mut self, writer: &mut impl Write, image: Option<ChartImage>) -> io::Result<()> {
        let Some(image) = image else {
            return self.clear(writer);
        };
        let are_pixels_cached = self.last_image.as_ref().is_some_and(|cached| {
            cached.width == image.width
                && cached.height == image.height
                && cached.rgba == image.rgba
        });
        if are_pixels_cached && self.is_placed && self.last_image.as_ref() == Some(&image) {
            return Ok(());
        }
        let next_slot = if are_pixels_cached || self.last_image.is_none() {
            self.active_slot
        } else {
            1 - self.active_slot
        };
        if !are_pixels_cached {
            self.pending_slot = Some(next_slot);
            Self::upload_image(
                writer,
                ImageUpload {
                    image_id: self.image_ids[next_slot],
                    image: &image,
                },
            )?;
        }
        Self::present_image(
            writer,
            ImagePlacement {
                image_id: self.image_ids[next_slot],
                area: image.area,
                visible_height: image.visible_height,
                retired_image_id: (next_slot != self.active_slot && self.last_image.is_some())
                    .then_some(self.image_ids[self.active_slot]),
            },
        )?;
        self.active_slot = next_slot;
        self.pending_slot = None;
        self.is_placed = true;
        self.last_image = Some(image);
        Ok(())
    }

    fn upload_image(writer: &mut impl Write, input: ImageUpload<'_>) -> io::Result<()> {
        let image = input.image;
        let payload = encode_base64(&image.rgba);
        let chunks = payload.as_bytes().chunks(4096);
        let count = chunks.len();
        for (index, chunk) in chunks.enumerate() {
            let more = usize::from(index + 1 < count);
            if index == 0 {
                write!(
                    writer,
                    "\x1b_Ga=t,t=d,f=32,q=2,i={},s={},v={},m={more};",
                    input.image_id, image.width, image.height
                )?;
            } else {
                write!(writer, "\x1b_Gq=2,m={more};")?;
            }
            writer.write_all(chunk)?;
            writer.write_all(b"\x1b\\")?;
        }
        writer.flush()
    }

    fn present_image(writer: &mut impl Write, input: ImagePlacement) -> io::Result<()> {
        let mut commands = Vec::new();
        queue!(
            commands,
            BeginSynchronizedUpdate,
            SavePosition,
            MoveTo(input.area.x, input.area.y)
        )?;
        // Cropping reveals the cached legend without changing the plot's pixels or scale.
        write!(
            commands,
            "\x1b_Ga=p,q=2,C=1,i={},p=1,c={},h={};\x1b\\",
            input.image_id, input.area.width, input.visible_height
        )?;
        if let Some(retired) = input.retired_image_id {
            write!(commands, "\x1b_Ga=d,d=I,i={retired},q=2;\x1b\\")?;
        }
        queue!(commands, RestorePosition, EndSynchronizedUpdate)?;
        let result = writer.write_all(&commands).and_then(|()| writer.flush());
        if result.is_err() {
            let _ = execute!(writer, RestorePosition, EndSynchronizedUpdate);
        }
        result
    }

    pub fn hide(&mut self, writer: &mut impl Write) -> io::Result<()> {
        if self.is_placed {
            write!(
                writer,
                "\x1b_Ga=d,d=i,i={},p=1,q=2;\x1b\\",
                self.image_ids[self.active_slot]
            )?;
            writer.flush()?;
            self.is_placed = false;
        }
        Ok(())
    }

    pub fn clear(&mut self, writer: &mut impl Write) -> io::Result<()> {
        let mut did_delete = false;
        for (slot, image_id) in self.image_ids.iter().enumerate() {
            if self.pending_slot == Some(slot)
                || (self.last_image.is_some() && slot == self.active_slot)
            {
                write!(writer, "\x1b_Ga=d,d=I,i={image_id},q=2;\x1b\\")?;
                did_delete = true;
            }
        }
        if did_delete {
            writer.flush()?;
        }
        self.last_image = None;
        self.pending_slot = None;
        self.is_placed = false;
        Ok(())
    }

    pub fn retain_result_in_scrollback(&mut self) {
        self.last_image = None;
        self.pending_slot = None;
        self.is_placed = false;
    }
}

struct ImageUpload<'a> {
    image_id: u32,
    image: &'a ChartImage,
}

struct ImagePlacement {
    image_id: u32,
    area: Rect,
    visible_height: u32,
    retired_image_id: Option<u32>,
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            encoded.push(if index > chunk.len() {
                '='
            } else {
                ALPHABET[((bits >> (18 - index * 6)) & 63) as usize] as char
            });
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn base64_matches_standard_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn image_updates_are_quiet_chunked_cached_and_deleted_only_by_owned_id() {
        let image = ChartImage {
            area: Rect::new(3, 4, 20, 5),
            width: 100,
            height: 50,
            visible_height: 50,
            rgba: vec![255; 100 * 50 * 4],
        };
        let mut graphics = KittyGraphics::create(1234);
        let mut output = vec![];
        graphics.update(&mut output, Some(image.clone())).unwrap();
        let text = String::from_utf8(output.clone()).unwrap();
        assert!(text.contains("a=t,t=d,f=32,q=2,i=1234,s=100,v=50,m=1"));
        assert!(text.contains("a=p,q=2,C=1,i=1234,p=1,c=20,h=50;"));
        let packets: Vec<_> = text.split("\x1b_G").skip(1).collect();
        assert!(packets.len() > 1);
        for packet in packets {
            let packet = packet.split("\x1b\\").next().unwrap();
            let (control, payload) = packet.split_once(';').unwrap();
            assert!(control.contains("q=2"));
            assert!(payload.len() <= 4096 && payload.len() % 4 == 0);
        }
        let length = output.len();
        graphics.update(&mut output, Some(image)).unwrap();
        assert_eq!(output.len(), length);
        graphics.update(&mut output, None).unwrap();
        assert_eq!(&output[length..], b"\x1b_Ga=d,d=I,i=1234,q=2;\x1b\\");
        let length = output.len();
        graphics.clear(&mut output).unwrap();
        assert_eq!(output.len(), length);
    }

    fn test_image(value: u8) -> ChartImage {
        ChartImage {
            area: Rect::new(3, 4, 20, 5),
            width: 100,
            height: 50,
            visible_height: 50,
            rgba: vec![value; 20_000],
        }
    }

    #[test]
    fn changed_images_upload_to_the_hidden_slot_before_an_atomic_swap() {
        let mut graphics = KittyGraphics::create(1234);
        let mut output = vec![];
        graphics.update(&mut output, Some(test_image(1))).unwrap();
        for value in 2..8 {
            output.clear();
            let previous = graphics.image_ids[graphics.active_slot];
            let next = graphics.image_ids[1 - graphics.active_slot];
            graphics
                .update(&mut output, Some(test_image(value)))
                .unwrap();
            let text = String::from_utf8(output.clone()).unwrap();
            assert!(text.starts_with(&format!("\x1b_Ga=t,t=d,f=32,q=2,i={next},")));
            let upload_end = text.find("q=2,m=0;").unwrap();
            let begin = text.find("\x1b[?2026h").unwrap();
            let place = text.find(&format!("a=p,q=2,C=1,i={next},")).unwrap();
            let delete = text.find(&format!("a=d,d=I,i={previous},")).unwrap();
            let end = text.find("\x1b[?2026l").unwrap();
            assert!(upload_end < begin && begin < place && place < delete && delete < end);
            assert!(!text.contains("a=T,"));
            assert!(graphics.pending_slot.is_none());
        }
    }

    #[test]
    fn legend_layout_changes_reuse_pixels_and_only_move_the_placement() {
        let mut graphics = KittyGraphics::create(u32::MAX);
        assert_eq!(graphics.image_ids, [u32::MAX, 1]);
        let mut output = vec![];
        let mut image = test_image(1);
        graphics.update(&mut output, Some(image.clone())).unwrap();
        output.clear();
        graphics.hide(&mut output).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("a=d,d=i,"));
        output.clear();
        image.area.y += 1;
        image.visible_height = 40;
        image.area.height = 4;
        graphics.update(&mut output, Some(image)).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(
            text.contains("a=p,")
                && text.contains("h=40;")
                && !text.contains(",Y=")
                && !text.contains(",r=")
        );
        assert!(!text.contains("a=t,") && !text.contains("a=d,d=I,"));
    }

    struct FailOnceWriter {
        remaining: Option<usize>,
        output: Vec<u8>,
    }

    impl Write for FailOnceWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == Some(0) {
                self.remaining = None;
                return Err(io::Error::other("test write failure"));
            }
            let count = self
                .remaining
                .map_or(bytes.len(), |remaining| remaining.min(bytes.len()));
            self.output.extend_from_slice(&bytes[..count]);
            self.remaining = self.remaining.map(|remaining| remaining - count);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_upload_does_not_commit_the_cache_and_both_owned_slots_are_cleaned() {
        let mut graphics = KittyGraphics::create(30);
        let original = test_image(1);
        graphics
            .update(&mut vec![], Some(original.clone()))
            .unwrap();
        let mut writer = FailOnceWriter {
            remaining: Some(100),
            output: vec![],
        };
        assert!(graphics.update(&mut writer, Some(test_image(2))).is_err());
        assert_eq!(graphics.last_image, Some(original));
        assert_eq!(graphics.pending_slot, Some(1));
        let mut cleanup = vec![];
        graphics.clear(&mut cleanup).unwrap();
        let cleanup = String::from_utf8(cleanup).unwrap();
        assert!(cleanup.contains("a=d,d=I,i=30,") && cleanup.contains("a=d,d=I,i=31,"));
        assert!(graphics.last_image.is_none() && graphics.pending_slot.is_none());
    }

    #[test]
    fn failed_swap_still_releases_synchronized_output() {
        let mut writer = FailOnceWriter {
            remaining: Some(20),
            output: vec![],
        };
        assert!(
            KittyGraphics::present_image(
                &mut writer,
                ImagePlacement {
                    image_id: 1,
                    area: Rect::new(3, 4, 20, 5),
                    visible_height: 50,
                    retired_image_id: Some(2),
                }
            )
            .is_err()
        );
        assert!(writer.output.ends_with(b"\x1b[?2026l"));
    }

    #[test]
    fn capability_checks_require_advertised_support_and_nonzero_pixel_dimensions() {
        assert!(KittyGraphics::supports_environment(GraphicsEnvironment {
            term: Some("xterm-kitty"),
            program: None,
            is_multiplexed: false
        }));
        assert!(KittyGraphics::supports_environment(GraphicsEnvironment {
            term: Some("xterm-256color"),
            program: Some("WezTerm"),
            is_multiplexed: false
        }));
        assert!(!KittyGraphics::supports_environment(GraphicsEnvironment {
            term: Some("xterm-256color"),
            program: None,
            is_multiplexed: false
        }));
        assert!(!KittyGraphics::supports_environment(GraphicsEnvironment {
            term: Some("xterm-kitty"),
            program: None,
            is_multiplexed: true
        }));
        assert_eq!(
            KittyGraphics::resolve_cell_pixels(WindowSize {
                columns: 80,
                rows: 24,
                width: 800,
                height: 480
            }),
            Some(CellPixels {
                width: 10,
                height: 20
            })
        );
        assert_eq!(
            KittyGraphics::resolve_cell_pixels(WindowSize {
                columns: 80,
                rows: 24,
                width: 0,
                height: 0
            }),
            None
        );
    }

    #[test]
    fn retained_result_images_are_not_deleted_on_successful_exit() {
        let mut graphics = KittyGraphics::create(55);
        let mut output = vec![];
        graphics
            .update(
                &mut output,
                Some(ChartImage {
                    area: Rect::new(0, 0, 1, 1),
                    width: 1,
                    height: 1,
                    visible_height: 1,
                    rgba: vec![0; 4],
                }),
            )
            .unwrap();
        let length = output.len();
        graphics.retain_result_in_scrollback();
        graphics.clear(&mut output).unwrap();
        assert_eq!(output.len(), length);
    }
}
