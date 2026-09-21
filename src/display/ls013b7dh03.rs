//! LS013B7DH03 display

use crate::{display::DisplayFrame, DisplayFrameChReceiver, DisplayFrameChSender};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use efm32xg_hal::{
    gpio::{OutPp, Pin},
    peripherals::Usart0,
    usart::spi::dma::Spi,
};
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Dimensions, Point, Size},
    pixelcolor::BinaryColor,
    primitives::Rectangle,
    Pixel,
};
use embedded_hal::digital::OutputPin;

/// The buffer size this driver needs
pub const BUF_SIZE: usize = HEIGHT * LINE_TOTAL_BYTE_COUNT;
/// The width, in pixels of the Ls013b7dh03 display
pub const WIDTH: usize = 128;
/// The height, in pixels of the Ls013b7dh03 display
pub const HEIGHT: usize = 128;
const LINE_WIDTH_BYTE_COUNT: usize = WIDTH / (u8::BITS as usize);
const LINE_PADDING_BYTE_COUNT: usize = 1;
const LINE_ADDRESS_BYTE_COUNT: usize = 1;
const LINE_TOTAL_BYTE_COUNT: usize =
    LINE_ADDRESS_BYTE_COUNT + LINE_WIDTH_BYTE_COUNT + LINE_PADDING_BYTE_COUNT;
/// LCD line filler byte
const FILLER_BYTE: u8 = 0xFF;

static mut BUFFER_0: UnsafeCell<[u8; BUF_SIZE]> = UnsafeCell::new([0; _]);
static BUFFER_0_AVAILABLE: AtomicBool = AtomicBool::new(true);
// static mut BUFFER_1: UnsafeCell<[u8; BUF_SIZE]> = UnsafeCell::new([0; _]);
// static BUFFER_1_AVAILABLE: AtomicBool = AtomicBool::new(true);

impl<'a> DisplayFrame<'a, BUF_SIZE> {
    /// Initialize the display frame bytes
    pub fn with_init(mut self, is_pixel_on: bool) -> Self {
        self.init(is_pixel_on);
        self
    }

    /// Initialize the internal buffer:
    /// - Write the on-wire address for each line, so that we only calculate them once
    /// - Set all pixels to given state
    /// - Write the filler byte a the end of each line, so that we don't have to do it ever again
    fn init(&mut self, is_pixel_on: bool) {
        let color = if is_pixel_on { 0x00 } else { 0xFF };
        // Write addresses and filler bytes to buffer
        for (addr, sl) in self
            .buffer
            .as_chunks_mut::<LINE_TOTAL_BYTE_COUNT>()
            .0
            .iter_mut()
            .enumerate()
        {
            // LCD address space starts at 1 for y
            sl[0] = ((addr + 1) as u8).reverse_bits();

            sl[1..(LINE_TOTAL_BYTE_COUNT - 1)]
                .iter_mut()
                .for_each(|b| *b = color);

            sl[LINE_TOTAL_BYTE_COUNT - 1] = FILLER_BYTE;
        }
    }

    /// Get the buffer index corresponding to a pixel coord, and its bitmask which shows which bit in the byte
    /// represents the pixel.
    #[inline(always)]
    fn get_pixel_addr_unchecked(&self, x: u8, y: u8) -> PAddr {
        let x = x as usize;
        let y = y as usize;
        assert!(x < WIDTH);
        assert!(y < HEIGHT);

        let col_byte = x / u8::BITS as usize;
        let col_bit = x % u8::BITS as usize;
        let index = (y * LINE_TOTAL_BYTE_COUNT) + (LINE_ADDRESS_BYTE_COUNT + col_byte);

        PAddr(
            index,
            // Pixel bits must be transmitted over SPI in reverse order,
            // so that's also their order in each byte of the buffer
            0x80 >> col_bit,
        )
    }

    /// Set the state of a pixel at the given coordinates
    fn write_unchecked(&mut self, x: u8, y: u8, is_pixel_on: bool) {
        let PAddr(index, bit_mask) = self.get_pixel_addr_unchecked(x, y);

        if ((self.buffer[index] & bit_mask) == 0) ^ is_pixel_on {
            // flip the pixel state
            self.buffer[index] ^= bit_mask;
        }
    }

    /// Read the state of a pixel at the given coordiantes
    #[allow(dead_code)]
    fn read_unchecked(&self, x: u8, y: u8) -> bool {
        let PAddr(index, bit_mask) = self.get_pixel_addr_unchecked(x, y);

        (self.buffer[index] & bit_mask) == 0
    }

    /// Invert the state of a pixel at the given coordinates, and return current state
    #[allow(dead_code)]
    fn flip_unchecked(&mut self, x: u8, y: u8) -> bool {
        let PAddr(index, bit_mask) = self.get_pixel_addr_unchecked(x, y);

        self.buffer[index] ^= bit_mask;

        (self.buffer[index] & bit_mask) == 0
    }
}

/// Pixel address in the undelying `u8` buffer of the [`DisplayFrame`]
///
/// (index, bit_mask)
struct PAddr(usize, u8);

impl<'a> Dimensions for DisplayFrame<'a, BUF_SIZE> {
    fn bounding_box(&self) -> Rectangle {
        Rectangle {
            top_left: Point { x: 0, y: 0 },
            size: Size {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        }
    }
}

impl<'a> DrawTarget for DisplayFrame<'a, BUF_SIZE> {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        const I_WIDTH: i32 = WIDTH as i32;
        const I_HEIGHT: i32 = HEIGHT as i32;

        // Check if the pixel coordinates are out of bounds (negative or greater than
        // (WIDTH,HEIGHT)). `DrawTarget` implementation are required to discard any out of bounds
        // pixels without returning an error or causing a panic.
        for Pixel(coord, color) in pixels.into_iter().filter(|Pixel(coord, _)| {
            coord.x >= 0 && coord.x < I_WIDTH && coord.y >= 0 && coord.y < I_HEIGHT
        }) {
            self.write_unchecked(coord.x as u8, coord.y as u8, color.is_on());
        }

        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.init(color.is_on());
        Ok(())
    }
}

/// Send frames to the LS013B7DH03 LCD
pub async fn lcd_task(
    frames_in: DisplayFrameChReceiver,
    frames_out: DisplayFrameChSender,
    mut spi: Spi<'static, Usart0>,
    mut cs: Pin<'D', 14, OutPp>,
    _disp_com_inv: Pin<'D', 13, OutPp>,
) {
    /// LCD Mode flags
    #[repr(u8)]
    enum LcdMode {
        Clear = 0x20,
        Update = 0x80,
    }

    // Clear display
    let _ = cs.set_high();
    let spi_ret = spi.transfer_async(&mut [], &[LcdMode::Clear as u8]).await;
    assert!(spi_ret.is_ok());
    let _ = cs.set_low();

    loop {
        let frame = frames_in.receive().await;

        let _ = cs.set_high();

        let spi_ret = spi.transfer_async(&mut [], &[LcdMode::Update as u8]).await;
        assert!(spi_ret.is_ok());

        let spi_ret = spi.transfer_async(&mut [], frame.as_bytes()).await;
        assert!(spi_ret.is_ok());

        let spi_ret = spi.transfer_async(&mut [], &[FILLER_BYTE]).await;
        assert!(spi_ret.is_ok());

        let _ = cs.set_low();

        frames_out.send(frame).await;
    }
}

/// Get all the statically allocated `DisplayFrame`s
///
/// # Panic
///
/// Panics if called twice
#[allow(static_mut_refs)]
pub fn take_display_frames<'a>() -> [DisplayFrame<'a, BUF_SIZE>; 1] {
    [
        if BUFFER_0_AVAILABLE.swap(false, Ordering::Relaxed) {
            DisplayFrame::new(
                // SAFETY: available can only be true once on one thread,
                // so there will only be at most one &mut reference
                unsafe { &mut *BUFFER_0.get() },
            )
            .with_init(false)
        } else {
            panic!("attempted to reuse BUFFER_0");
        },
        // if BUFFER_1_AVAILABLE.swap(false, Ordering::Relaxed) {
        //     DisplayFrame::new(
        //         // SAFETY: available can only be true once on one thread,
        //         // so there will only be at most one &mut reference
        //         unsafe { &mut *BUFFER_1.get() },
        //     )
        //     .with_init(false)
        // } else {
        //     panic!("attempted to reuse BUFFER_1");
        // },
    ]
}
