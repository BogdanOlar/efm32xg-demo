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

/// LCD Mode flags
#[derive(Debug)]
#[repr(u8)]
enum LcdMode {
    Clear = 0x20,
    Update = 0x80,
}

/// Get all the statically allocated `DisplayFrame`s
///
/// # Panic
///
/// Panics if called twice
pub fn take_display_frames<'a>() -> [DisplayFrame<'a, BUF_SIZE>; 1] {
    // Initialize the display buffers before returning
    [
        DisplayFrame::new(if BUFFER_0_AVAILABLE.swap(false, Ordering::Relaxed) {
            // SAFETY: available can only be true once on one thread,
            // so there will only be at most one &mut reference
            let buffer = unsafe { &mut *&raw mut BUFFER_0 };
            buffer.get_mut()
        } else {
            panic!("attempted to reuse BUFFER_0");
        })
        .with_init(false),
        // DisplayFrame::new(if BUFFER_1_AVAILABLE.swap(false, Ordering::Relaxed) {
        //     // SAFETY: available can only be true once on one thread,
        //     // so there will only be at most one &mut reference
        //     let buffer = unsafe { &mut *&raw mut BUFFER_1 };
        //     buffer.get_mut()
        // } else {
        //     panic!("attempted to reuse BUFFER_1");
        // })
        // .with_init(false),
    ]
}

impl<'a> DisplayFrame<'a, BUF_SIZE> {
    /// Initialize the display frame bytes
    pub fn with_init(mut self, is_pixel_on: bool) -> Self {
        self.init(false);
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
            .chunks_exact_mut(LINE_TOTAL_BYTE_COUNT)
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
    fn get_pixel_addr_unchecked(&self, x: u8, y: u8) -> (usize, u8) {
        assert!((x as usize) < WIDTH);
        assert!((y as usize) < HEIGHT);

        let col_byte = x as usize / u8::BITS as usize;
        let col_bit = x as usize % u8::BITS as usize;
        let index = (y as usize * LINE_TOTAL_BYTE_COUNT) + (LINE_ADDRESS_BYTE_COUNT + col_byte);

        // Pixel bits must be transmitted over SPI in reverse order,
        // so that's also their order in each byte of the buffer
        (index, 0x80 >> col_bit)
    }

    /// Set the state of a pixel at the given coordinates
    fn write(&mut self, x: u8, y: u8, is_pixel_on: bool) {
        let (index, bit_mask) = self.get_pixel_addr_unchecked(x, y);

        if ((self.buffer[index] & bit_mask) == 0) ^ is_pixel_on {
            // flip the pixel state
            self.buffer[index] ^= bit_mask;
        }
    }

    /// Read the state of a pixel at the given coordiantes
    fn read(&self, x: u8, y: u8) -> bool {
        let (index, bit_mask) = self.get_pixel_addr_unchecked(x, y);

        (self.buffer[index] & bit_mask) == 0
    }

    /// Invert the state of a pixel at the given coordinates, and return current state
    fn flip(&mut self, x: u8, y: u8) -> bool {
        let (index, bit_mask) = self.get_pixel_addr_unchecked(x, y);

        self.buffer[index] ^= bit_mask;

        (self.buffer[index] & bit_mask) == 0
    }
}

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
        // Check if the pixel coordinates are out of bounds (negative or greater than
        // (WIDTH,HEIGHT)). `DrawTarget` implementation are required to discard any out of bounds
        // pixels without returning an error or causing a panic.
        for (x, y, is_pixel_on) in pixels
            .into_iter()
            .filter(|p| p.0.x >= 0 && p.0.x < WIDTH as i32 && p.0.y >= 0 && p.0.y < HEIGHT as i32)
            .map(|p| (p.0.x as u8, p.0.y as u8, p.1.is_on()))
        {
            self.write(x, y, is_pixel_on);
            // self.flip(x, y);
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
    // Clear display
    let spi_ret = spi.transfer_async(&mut [], &[LcdMode::Clear as u8]).await;
    assert!(spi_ret.is_ok());

    loop {
        let buffer = frames_in.receive().await;

        // Assert CS
        let _ = cs.set_high();

        // Write update command
        let spi_ret = spi.transfer_async(&mut [], &[LcdMode::Update as u8]).await;
        assert!(spi_ret.is_ok());

        // Write buffer
        let spi_ret = spi.transfer_async(&mut [], buffer.as_bytes()).await;
        assert!(spi_ret.is_ok());

        // Write filler byte
        let spi_ret = spi.transfer_async(&mut [], &[FILLER_BYTE]).await;
        assert!(spi_ret.is_ok());

        // Deassert CS
        let _ = cs.set_low();

        frames_out.send(buffer).await;
    }
}
