//! Short CPU benchmarks used to ground analytical model estimates.

use std::{
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

/// Results from the bounded memory-copy and f32 GEMM benchmarks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Benchmarks {
    /// Best observed copy/read throughput in decimal GB/s.
    pub memory_bandwidth_gbps: f64,
    /// Best observed f32 matrix multiplication throughput in GFLOP/s.
    pub gemm_gflops: f64,
    /// Total time spent in the benchmark, milliseconds.
    pub elapsed_ms: u64,
}

/// Run a multi-threaded copy/read benchmark and f32 GEMM, each for at most three samples.
///
/// Call this from a blocking thread. The working set is bounded and the requested work finishes
/// well under two seconds on supported desktop hardware.
pub fn run() -> Benchmarks {
    let started = Instant::now();
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 16);
    let memory_bandwidth_gbps = measure_memory_bandwidth(threads);
    let gemm_gflops = measure_gemm(threads);
    Benchmarks {
        memory_bandwidth_gbps,
        gemm_gflops,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }
}

fn measure_memory_bandwidth(threads: usize) -> f64 {
    const BYTES: usize = 64 * 1024 * 1024;
    let source = vec![0x5au8; BYTES];
    let mut destination = vec![0u8; BYTES];
    let chunk_len = BYTES.div_ceil(threads);
    let mut best = Duration::MAX;
    let mut checksum = 0u64;

    let barrier = Arc::new(Barrier::new(threads + 1));
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, output) in destination.chunks_mut(chunk_len).enumerate() {
            let start = chunk_index * chunk_len;
            let input = &source[start..start + output.len()];
            let barrier = Arc::clone(&barrier);
            handles.push(scope.spawn(move || {
                let mut checksum = 0u64;
                for _ in 0..3 {
                    barrier.wait();
                    output.copy_from_slice(input);
                    checksum = checksum
                        .wrapping_add(output.iter().map(|byte| u64::from(*byte)).sum::<u64>());
                    barrier.wait();
                }
                checksum
            }));
        }
        for _ in 0..3 {
            let started = Instant::now();
            barrier.wait();
            barrier.wait();
            best = best.min(started.elapsed());
        }
        for handle in handles {
            checksum = checksum.wrapping_add(handle.join().unwrap_or_default());
        }
    });
    std::hint::black_box(checksum);
    let seconds = best.as_secs_f64().max(0.000_001);
    (BYTES as f64 * 3.0 / seconds) / 1_000_000_000.0
}

fn measure_gemm(threads: usize) -> f64 {
    const N: usize = 384;
    let a: Vec<f32> = (0..N * N)
        .map(|index| (index % 101) as f32 / 101.0)
        .collect();
    let b: Vec<f32> = (0..N * N).map(|index| (index % 89) as f32 / 89.0).collect();
    let mut c = vec![0.0f32; N * N];
    let rows_per_thread = N.div_ceil(threads);
    let mut best = Duration::MAX;

    for _ in 0..3 {
        c.fill(0.0);
        let started = Instant::now();
        std::thread::scope(|scope| {
            for (chunk_index, c_rows) in c.chunks_mut(rows_per_thread * N).enumerate() {
                let row_start = chunk_index * rows_per_thread;
                let rows = c_rows.len() / N;
                let a_rows = &a[row_start * N..(row_start + rows) * N];
                let b_rows = &b;
                scope.spawn(move || {
                    // Row-major operands: row stride N, column stride 1. `a_rows`/`c_rows` hold exactly
                    // `rows × N` elements and `b_rows` holds `N × N`, so every access stays inside its
                    // slice; the C row blocks are disjoint across threads. (Column-major strides here
                    // used to read and write far outside the slices — a crash on Windows.)
                    debug_assert_eq!(a_rows.len(), rows * N);
                    debug_assert_eq!(c_rows.len(), rows * N);
                    debug_assert_eq!(b_rows.len(), N * N);
                    unsafe {
                        matrixmultiply::sgemm(
                            rows,
                            N,
                            N,
                            1.0,
                            a_rows.as_ptr(),
                            N as isize,
                            1,
                            b_rows.as_ptr(),
                            N as isize,
                            1,
                            0.0,
                            c_rows.as_mut_ptr(),
                            N as isize,
                            1,
                        );
                    }
                });
            }
        });
        best = best.min(started.elapsed());
    }
    std::hint::black_box(c[0]);
    let seconds = best.as_secs_f64().max(0.000_001);
    (2.0 * (N as f64).powi(3) / seconds) / 1_000_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the GEMM probe once passed column-major strides for row-major slices and read and
    /// wrote outside them (access violation on Windows). Runs the real probe with several thread
    /// counts so CI on every OS exercises it.
    #[test]
    fn gemm_probe_stays_in_bounds_for_any_thread_count() {
        for threads in [1, 2, 3, 7, 8, 16] {
            let gflops = measure_gemm(threads);
            assert!(
                gflops.is_finite() && gflops > 0.0,
                "threads={threads}: {gflops}"
            );
        }
    }

    /// The strides must describe row-major data: check one product against a naive multiply.
    #[test]
    fn row_major_strides_compute_the_right_product() {
        const M: usize = 3;
        const N: usize = 4;
        let a: Vec<f32> = (0..M * N).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..N * N).map(|i| (i % 5) as f32).collect();
        let mut c = vec![0.0f32; M * N];
        unsafe {
            matrixmultiply::sgemm(
                M,
                N,
                N,
                1.0,
                a.as_ptr(),
                N as isize,
                1,
                b.as_ptr(),
                N as isize,
                1,
                0.0,
                c.as_mut_ptr(),
                N as isize,
                1,
            );
        }
        for i in 0..M {
            for j in 0..N {
                let expected: f32 = (0..N).map(|k| a[i * N + k] * b[k * N + j]).sum();
                assert_eq!(c[i * N + j], expected);
            }
        }
    }
}
