#![forbid(unsafe_code)]
#![warn(missing_debug_implementations, rust_2018_idioms)]

#[doc(inline)]
pub use
{
    async_executor::{ Executor, LocalExecutor, Task },
    async_io::{ block_on, Async, Timer },
    blocking::{ unblock, Unblock },
    futures_lite::{ future, io, pin, prelude, ready, stream },
};

#[doc(inline)]
pub use
{
    async_channel as channel,
    async_fs as fs,
    async_lock as lock,
    async_net as net,
    async_process as process,
};

mod spawn;
pub use crate::spawn::spawn;
