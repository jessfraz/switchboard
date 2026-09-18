use switchboard_core::{Error, Failure, FailurePhase, Result};

pub(super) const READ_CONCURRENCY: usize = 4;

/// Keep results in source order, including sources skipped after auth fails.
/// Callers may consume one wave at a time to check deadlines or source identity
/// before scheduling the next wave.
pub(super) fn read_messages<S: Sync, T: Send>(
    sources: &[S],
    blocked: &mut bool,
    worker_error: &'static str,
    read: impl Fn(&S) -> Result<T> + Sync,
) -> Vec<Result<Option<T>>> {
    let mut results = Vec::with_capacity(sources.len());
    for chunk in sources.chunks(READ_CONCURRENCY) {
        if *blocked {
            results.extend(chunk.iter().map(|_| Ok(None)));
            continue;
        }
        std::thread::scope(|scope| {
            let read = &read;
            let handles = chunk
                .iter()
                .map(|source| scope.spawn(move || read(source).map(Some)))
                .collect::<Vec<_>>();
            for handle in handles {
                let result = handle
                    .join()
                    .unwrap_or_else(|_| Err(Error::Execution(worker_error.into())));
                *blocked |= result
                    .as_ref()
                    .err()
                    .is_some_and(|error| Failure::from_error(error).phase == FailurePhase::Authentication);
                results.push(result);
            }
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn concurrent_reads_preserve_order_and_stop_after_an_authentication_failure() {
        let started = AtomicUsize::new(0);
        let mut blocked = false;
        let results = read_messages(&[0, 1, 2, 3, 4, 5], &mut blocked, "worker failed", |source| {
            started.fetch_add(1, Ordering::SeqCst);
            if *source == 1 {
                Err(Error::AuthenticationRejected {
                    reason: "test rejection".into(),
                })
            } else {
                Ok(*source)
            }
        });
        assert!(blocked);
        assert_eq!(started.load(Ordering::SeqCst), READ_CONCURRENCY);
        assert_eq!(results.len(), 6);
        assert_eq!(results[0].as_ref().expect("read should succeed"), &Some(0));
        assert!(matches!(results[1], Err(Error::AuthenticationRejected { .. })));
        assert_eq!(results[2].as_ref().expect("read should succeed"), &Some(2));
        assert_eq!(results[3].as_ref().expect("read should succeed"), &Some(3));
        assert!(results[4..].iter().all(|result| matches!(result, Ok(None))));
    }

    #[test]
    fn worker_failure_retains_other_results_and_does_not_block_later_reads() {
        let mut blocked = false;
        let results = read_messages(&[0, 1, 2, 3, 4], &mut blocked, "worker failed", |source| {
            if *source == 1 {
                panic!("test worker failure");
            }
            Ok(*source)
        });
        assert!(!blocked);
        assert_eq!(results.len(), 5);
        assert!(matches!(&results[1], Err(Error::Execution(error)) if error == "worker failed"));
        for index in [0, 2, 3, 4] {
            assert_eq!(results[index].as_ref().expect("read should succeed"), &Some(index));
        }
    }
}
