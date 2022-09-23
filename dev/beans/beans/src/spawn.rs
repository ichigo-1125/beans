use std::future::Future;
use std::panic::catch_unwind;
use std::thread;

use async_executor::{Executor, Task};
use async_io::block_on;
use futures_lite::future;
use once_cell::sync::Lazy;

//------------------------------------------------------------------------------
//  spawn
//------------------------------------------------------------------------------
pub fn spawn<T, F>(future: F) -> Task<T>
where
	F: Future<Output = T> + Send + 'static,
	T: Send + 'static,
{
	static GLOBAL: Lazy<Executor<'_>> = Lazy::new(|| {
		let num_threads = {
			std::env::var("SMOL_THREADS")
				.ok()
				.and_then(|s| s.parse().ok())
				.unwrap_or(1)
		};

		for n in 1..=num_threads
		{
			thread::Builder::new()
				.name(format!("smol-{}", n))
				.spawn(|| {
					loop
					{
						catch_unwind(|| block_on(GLOBAL.run(future::pending::<()>()))).ok();
					}
				})
				.expect("cannot spawn executor thread");
		}

		Executor::new()
	});

	GLOBAL.spawn(future)
}
