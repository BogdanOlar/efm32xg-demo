//! Display

pub mod ls013b7dh03 {
    use core::{
        cell::UnsafeCell,
        sync::atomic::{AtomicBool, Ordering},
    };

    static mut BUFFER: UnsafeCell<[u8; BUF_SIZE]> = UnsafeCell::new([0; _]);
    static BUFFER_AVAILABLE: AtomicBool = AtomicBool::new(true);

    /// The buffer size this driver needs
    pub const BUF_SIZE: usize = DisplayFrame::HEIGHT * LINE_TOTAL_BYTE_COUNT;
    /// Display filler byte
    pub const FILLER_BYTE: u8 = 0xFF;

    const LINE_WIDTH_BYTE_COUNT: usize = DisplayFrame::WIDTH / (u8::BITS as usize);
    const LINE_PADDING_BYTE_COUNT: usize = 1;
    const LINE_ADDRESS_BYTE_COUNT: usize = 1;
    const LINE_TOTAL_BYTE_COUNT: usize =
        LINE_ADDRESS_BYTE_COUNT + LINE_WIDTH_BYTE_COUNT + LINE_PADDING_BYTE_COUNT;

    /// LCD Mode flags
    #[derive(Debug)]
    #[repr(u8)]
    pub enum LcdMode {
        Clear = 0x20,
        Update = 0x80,
    }

    pub fn take_display_frame() -> DisplayFrame<'static> {
        let available = BUFFER_AVAILABLE.swap(false, Ordering::Relaxed);
        if available {
            // SAFETY: available can only be true once on one thread,
            // so there will only be at most one &mut reference
            let buffer = unsafe { &mut *&raw mut BUFFER };
            DisplayFrame::new(buffer.get_mut())
        } else {
            panic!("attempted to reuse LS013B7DH03_BUFFER");
        }
    }

    pub struct DisplayFrame<'a> {
        buffer: &'a mut [u8; BUF_SIZE],
    }

    impl<'a> DisplayFrame<'a> {
        /// The width, in pixels of the Ls013b7dh03 display
        pub const WIDTH: usize = 128;

        /// The height, in pixels of the Ls013b7dh03 display
        pub const HEIGHT: usize = 128;

        pub fn new(buffer: &'a mut [u8; BUF_SIZE]) -> Self {
            Self { buffer }.reset()
        }

        /// Initialize the internal buffer:
        /// - Write the on-wire address for each line, so that we only calculate them once
        /// - Set all pixels to OFF state (which corresponds to bit `1`)
        /// - Write the filler byte a the end of each line, so that we don't have to do it ever again
        fn reset(self) -> Self {
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
                    .for_each(|b| *b = 0xFF);

                sl[LINE_TOTAL_BYTE_COUNT - 1] = FILLER_BYTE;
            }

            self
        }

        pub fn width(&self) -> usize {
            Self::WIDTH
        }

        pub fn height(&self) -> usize {
            Self::HEIGHT
        }

        pub fn buffer(&self) -> &[u8; 2304] {
            self.buffer
        }

        /// Get the buffer index corresponding to a pixel coord, and its bitmask which shows which bit in the byte
        /// represents the pixel.
        fn get_pixel_addr_unchecked(&self, x: u8, y: u8) -> (usize, u8) {
            assert!((x as usize) < Self::WIDTH);
            assert!((y as usize) < Self::HEIGHT);

            let col_byte = x as usize / u8::BITS as usize;
            let col_bit = x as usize % u8::BITS as usize;
            let index = (y as usize * LINE_TOTAL_BYTE_COUNT) + (LINE_ADDRESS_BYTE_COUNT + col_byte);

            // Pixel bits must be transmitted over SPI in reverse order,
            // so that's also their order in each byte of the buffer
            (index, 0x80 >> col_bit)
        }

        /// Set the state of a pixel at the given coordinates
        pub fn write(&mut self, x: u8, y: u8, is_pixel_on: bool) {
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
}
