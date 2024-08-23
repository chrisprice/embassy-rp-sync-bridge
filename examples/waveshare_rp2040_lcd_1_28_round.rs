#![no_std]
#![no_main]

use core::cell::RefCell;

use defmt::*;
use eg_seven_segment::{Digit, SevenSegmentStyleBuilder};
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embassy_executor::Spawner;
use embassy_rp::{
    clocks,
    gpio::{Level, Output},
    i2c::I2c,
    multicore::Stack,
    spi::{Blocking, Spi},
};
use embassy_rp_sync_bridge::State;
use embassy_sync::blocking_mutex::{raw::NoopRawMutex, Mutex};
use embassy_time::{Duration, Ticker};
use embedded_hal::delay::DelayNs;
use gc9a01::{prelude::DisplayResolution240x240, Gc9a01, SPIDisplayInterface};
use qmi8658::{
    command::register::{
        acceleration::{AccelerationOutput, AngularRateOutput},
        ctrl2::Ctrl2Register,
        ctrl3::Ctrl3Register,
    },
    Qmi8658,
};
use static_cell::{ConstStaticCell, StaticCell};
use {defmt_rtt as _, panic_probe as _};

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyleBuilder, Rectangle, StrokeAlignment},
};

use gc9a01::prelude::*;

struct ImuData {
    acceleration: AccelerationOutput,
    angular_rate: AngularRateOutput,
    temperature: f32,
}

static CORE1_STACK: ConstStaticCell<Stack<196608>> = ConstStaticCell::new(Stack::new());
static STATE: StaticCell<State<u32, ImuData, 1, 1>> = StaticCell::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("Hello, world!");
    let p = embassy_rp::init(Default::default());

    let i2c = p.I2C1;
    let scl_pin = p.PIN_7;
    let sda_pin = p.PIN_6;
    let spi = p.SPI1;
    let clk_pin = p.PIN_10;
    let mosi_pin = p.PIN_11;
    let cs_output = Output::new(p.PIN_9, Level::Low);
    let dc_output = Output::new(p.PIN_8, Level::Low);
    let mut reset_output = Output::new(p.PIN_12, Level::High);
    let mut backlight_output = Output::new(p.PIN_25, Level::Low);

    let core1_stack = CORE1_STACK.take();
    let state = STATE.init(State::new());

    let (tx, rx) =
        embassy_rp_sync_bridge::spawn(p.CORE1, core1_stack, state, move |bidi_channel| {
            let i2c = I2c::new_blocking(i2c, scl_pin, sda_pin, embassy_rp::i2c::Config::default());
            let delay = embassy_time::Delay;
            let mut imu = Qmi8658::new_secondary_address(i2c, delay);

            // enable the sensors and initialise their respective control registers
            imu.set_sensors_enable(true, true).unwrap();
            imu.set_crtl2(Ctrl2Register(0x00)).unwrap();
            imu.set_crtl3(Ctrl3Register(0x00)).unwrap();

            let mut display_config = embassy_rp::spi::Config::default();
            display_config.frequency = clocks::clk_peri_freq() / 2;

            let spi: Spi<'_, _, Blocking> =
                Spi::new_blocking_txonly(spi, clk_pin, mosi_pin, display_config.clone());

            // NoopRawMutex can be used as the bus isn't shared between tasks/cores.
            // N.B. This is an blocking_mutex.
            let spi_bus: Mutex<NoopRawMutex, _> = Mutex::new(RefCell::new(spi));
            let spi_device = SpiDeviceWithConfig::new(&spi_bus, cs_output, display_config);

            let interface = SPIDisplayInterface::new(spi_device, dc_output);

            let mut display = Gc9a01::new(
                interface,
                DisplayResolution240x240,
                DisplayRotation::Rotate0,
            )
            .into_buffered_graphics();

            // Reset the display, clear it and then turn on the backlight.
            // This avoids showing whatever was in the RAM before the reset.
            let mut delay = embassy_time::Delay;
            display.reset(&mut reset_output, &mut delay).unwrap();
            display.init(&mut delay).unwrap();
            display.clear();
            display.flush().unwrap();
            delay.delay_ms(20);
            backlight_output.set_high();

            let screen_width = display.size().width;
            let screen_height = display.size().height;
            let digit_width = 48;
            let digit_height = digit_width * 2;
            let digit_spacing = 10;
            let segment_width = 10;

            let style = SevenSegmentStyleBuilder::new()
                .digit_size((digit_width, digit_height).into())
                .digit_spacing(digit_spacing)
                .segment_width(segment_width)
                .segment_color(Rgb565::CSS_DARK_GRAY)
                .build();

            loop {
                // Send the IMU data if there's space otherwise skip sending
                let _ = bidi_channel.send(ImuData {
                    acceleration: imu.get_acceleration().unwrap(),
                    angular_rate: imu.get_angular_rate().unwrap(),
                    temperature: imu.get_temperature().unwrap(),
                });

                // Draw the value if there's a value to receive otherwise skip drawing
                let Ok(value) = bidi_channel.receive() else {
                    continue;
                };

                // Clear only the area where the digits will be drawn
                Rectangle::with_center(
                    (screen_width as i32 / 2, screen_height as i32 / 2).into(),
                    (4 * digit_width + 3 * digit_spacing, digit_height).into(),
                )
                .into_styled(
                    PrimitiveStyleBuilder::new()
                        .fill_color(Rgb565::BLACK)
                        // HACK: work around the off by 1 error
                        .stroke_color(Rgb565::BLACK)
                        .stroke_width(1)
                        .stroke_alignment(StrokeAlignment::Outside)
                        .build(),
                )
                .draw(&mut display)
                .unwrap();

                let mut next: Point = (
                    (screen_width - 4 * digit_width - 3 * digit_spacing) as i32 / 2,
                    (screen_height - digit_height) as i32 / 2,
                )
                    .into();
                for i in 0..4 {
                    next = Digit::new(
                        char::from_digit(value / 10_u32.pow(3 - i) % 10, 10)
                            .unwrap()
                            .try_into()
                            .unwrap(),
                        next,
                    )
                    .into_styled(style)
                    .draw(&mut display)
                    .unwrap();
                }
                display.flush().unwrap();
            }
        });

    let mut ticker = Ticker::every(Duration::from_hz(10));
    loop {
        let data = rx.receive().await;
        info!(
            "Received: accel x/y/z: {}/{}/{}, angular rate x/y/z: {}/{}/{}, temperature: {}",
            data.acceleration.x,
            data.acceleration.y,
            data.acceleration.z,
            data.angular_rate.x,
            data.angular_rate.y,
            data.angular_rate.z,
            data.temperature
        );

        // Send the y acceleration value to core 1
        let mut y = data.acceleration.y * 1000.;
        if y < 0. {
            y *= -1.;
        }
        tx.send(y as u32).await;
        ticker.next().await;
    }
}
