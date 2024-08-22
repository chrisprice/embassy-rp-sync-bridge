#![no_std]

use cortex_m::{delay::Delay, Peripherals};
use embassy_rp::{
    clocks::clk_sys_freq,
    multicore::{self, Stack},
    peripherals::CORE1,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError},
};

pub struct State<T, U, const N: usize, const M: usize> {
    tx: Channel<CriticalSectionRawMutex, T, N>,
    rx: Channel<CriticalSectionRawMutex, U, M>,
}

impl<T, U, const N: usize, const M: usize> Default for State<T, U, N, M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, U, const N: usize, const M: usize> State<T, U, N, M> {
    pub fn new() -> Self {
        Self {
            tx: Channel::new(),
            rx: Channel::new(),
        }
    }
}

pub struct BidiChannel<'a, T, U, const N: usize, const M: usize> {
    tx: Sender<'a, CriticalSectionRawMutex, U, M>,
    rx: Receiver<'a, CriticalSectionRawMutex, T, N>,
}

impl<'a, T, U, const N: usize, const M: usize> BidiChannel<'a, T, U, N, M> {
    pub fn send(&self, item: U) -> Result<(), TrySendError<U>> {
        self.tx.try_send(item)
    }

    pub fn receive(&self) -> Result<T, TryReceiveError> {
        self.rx.try_receive()
    }
}

pub fn spawn<F, T, U, const N: usize, const M: usize, const S: usize>(
    core1: CORE1,
    stack: &'static mut Stack<S>,
    state: &'static mut State<T, U, N, M>,
    func: F,
) -> (
    Sender<'static, CriticalSectionRawMutex, T, N>,
    Receiver<'static, CriticalSectionRawMutex, U, M>,
)
where
    F: FnOnce(BidiChannel<'static, T, U, N, M>, Delay) -> bad::Never + Send + 'static,
    T: Send,
    U: Send,
{
    let State { tx, rx } = state;

    let bidi_channel = BidiChannel {
        tx: rx.sender(),
        rx: tx.receiver(),
    };

    multicore::spawn_core1(core1, stack, move || {
        // SAFETY: embassy-rp is not using the SYST peripheral
        let syst = unsafe { Peripherals::steal() }.SYST;
        let delay = Delay::new(syst, clk_sys_freq());
        func(bidi_channel, delay)
    });

    (tx.sender(), rx.receiver())
}

// https://github.com/nvzqz/bad-rs/blob/master/src/never.rs
mod bad {
    pub(crate) type Never = <F as HasOutput>::Output;

    pub trait HasOutput {
        type Output;
    }

    impl<O> HasOutput for fn() -> O {
        type Output = O;
    }

    type F = fn() -> !;
}
