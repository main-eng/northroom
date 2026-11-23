//! Runs work over a list across threads, keeping results in item order.

use std::sync::atomic::{AtomicUsize, Ordering};

/// How work is spread over threads: at most `most_threads` (capped at the machine's
/// parallelism), each with at least `fewest_items_per_thread` items, so a short list runs on the
/// calling thread alone.
#[derive(Debug, Clone, Copy)]
pub struct Spread {
    pub most_threads: usize,
    pub fewest_items_per_thread: usize,
}

/// Runs `work` on each of `items` as `spread` allows and returns results in item order. Each
/// thread takes the next free item, so a few large items do not hold up the rest.
pub fn each_on_threads<T: Sync, R: Send>(
    items: &[T],
    spread: Spread,
    work: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(spread.most_threads)
        .min(items.len().div_ceil(spread.fewest_items_per_thread.max(1)));
    if threads <= 1 {
        return items.iter().map(work).collect();
    }
    let next = AtomicUsize::new(0);
    let (work, next) = (&work, &next);
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return done;
                        };
                        done.push((index, work(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    done.sort_unstable_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPREAD: Spread = Spread {
        most_threads: 4,
        fewest_items_per_thread: 2,
    };

    #[test]
    fn keeps_each_result_in_its_items_place() {
        let items: Vec<u64> = (0..1000).collect();
        // Uneven work, so the threads finish out of order.
        let squares = each_on_threads(&items, SPREAD, |item| {
            if item % 7 == 0 {
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
            item * item
        });
        assert_eq!(
            squares,
            items.iter().map(|item| item * item).collect::<Vec<_>>()
        );
    }

    #[test]
    fn does_a_short_list_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let threads = each_on_threads(&[1, 2], SPREAD, |_| std::thread::current().id());
        assert_eq!(threads, vec![caller, caller]);
        assert!(each_on_threads(&[] as &[u8], SPREAD, |item| *item).is_empty());
    }
}
