//! Display

pub mod ls013b7dh03;

use crate::{DisplayFrameChReceiver, DisplayFrameChSender};
use defmt::error;
use embassy_time::Instant;
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Dimensions, Point},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    primitives::{Primitive, PrimitiveStyle, PrimitiveStyleBuilder, StrokeAlignment, Triangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
    Drawable,
};
use heapless::format;

/// Generic display frame, parametrized over the size of the undelying `u8` buffer (`N: usize`)
pub struct DisplayFrame<'a, const N: usize> {
    pub buffer: &'a mut [u8; N],
}

impl<'a, const N: usize> DisplayFrame<'a, N> {
    pub fn new(buffer: &'a mut [u8; N]) -> Self {
        Self { buffer }
    }

    /// Get the raw bytes of the display frame.
    ///
    /// These include the line address bytes that the LCD expects
    pub fn as_bytes(&self) -> &[u8] {
        self.buffer.as_slice()
    }
}

pub async fn display_task(frames_in: DisplayFrameChReceiver, frames_out: DisplayFrameChSender) {
    // Create styles used by the drawing operations.
    let thin_stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let border_stroke = PrimitiveStyleBuilder::new()
        .stroke_color(BinaryColor::On)
        .stroke_width(3)
        .stroke_alignment(StrokeAlignment::Inside)
        .build();
    let character_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::Off);
    let left_aligned = TextStyleBuilder::new()
        .alignment(Alignment::Left)
        .baseline(Baseline::Top)
        .build();
    let mut y_offset = 20;

    let sec_us: f32 = 1000000.0;
    let mut prev_frame_start = Instant::now();

    loop {
        let mut frame = frames_in.receive().await;
        let frame_start = Instant::now();
        let frame_us = (frame_start - prev_frame_start).as_micros() as f32;
        prev_frame_start = frame_start;

        frame.clear(BinaryColor::Off);

        // Draw a 3px wide outline around the display.
        frame
            .bounding_box()
            .into_styled(border_stroke)
            .draw(&mut frame);

        // Draw a triangle.
        Triangle::new(
            Point::new(100, 16 + y_offset),
            Point::new(100 + 16, 16 + y_offset),
            Point::new(100 + 8, y_offset),
        )
        .into_styled(thin_stroke)
        .draw(&mut frame);
        y_offset = if y_offset <= -16 {
            ls013b7dh03::HEIGHT as i32
        } else {
            y_offset - 1
        };

        // FPS
        let fps = if frame_us.is_normal() {
            sec_us / frame_us
        } else {
            0.0
        };

        if let Ok(fps_str) = format!(10; "{:.1} fps", fps) {
            let fps_txt = Text::with_text_style(
                fps_str.as_str(),
                frame.bounding_box().top_left,
                character_style,
                left_aligned,
            );
            frame.fill_solid(&fps_txt.bounding_box(), BinaryColor::On);
            fps_txt.draw(&mut frame);
        } else {
            error!("Could not format fps {}", &fps);
        }

        frames_out.send(frame).await;
    }
}
