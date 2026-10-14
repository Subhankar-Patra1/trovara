//! Vectors are stored as 768 × float16 (1.5 KB each), kept that way in memory too, and scored as float32 (TRD §8.4):
//! half the memory of float32 for a few milliseconds more per query.

use half::f16;
use rayon::prelude::*;

pub const DIMENSION: usize = 768;

/// A vector as the index stores it: 768 little-endian float16s.
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|&x| f16::from_f32(x).to_le_bytes())
        .collect()
}

pub fn from_blob(blob: &[u8]) -> Option<Vec<f32>> {
    (blob.len() == DIMENSION * 2).then(|| {
        blob.chunks_exact(2)
            .map(|pair| f16::from_le_bytes([pair[0], pair[1]]).to_f32())
            .collect()
    })
}

/// Every vector search scores, as rows of float16 bits.
pub struct Matrix {
    rows: usize,
    halves: Vec<u16>,
}

impl Matrix {
    pub fn with_capacity(rows: usize) -> Matrix {
        Matrix {
            rows: 0,
            halves: Vec::with_capacity(rows * DIMENSION),
        }
    }

    /// Adds one row from a stored blob. False (and nothing added) when it isn't a vector.
    pub fn push_blob(&mut self, blob: &[u8]) -> bool {
        if blob.len() != DIMENSION * 2 {
            return false;
        }
        self.halves.extend(
            blob.chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]])),
        );
        self.rows += 1;
        true
    }

    pub fn push(&mut self, vector: &[f32]) {
        assert_eq!(vector.len(), DIMENSION);
        self.halves
            .extend(vector.iter().map(|&x| f16::from_f32(x).to_bits()));
        self.rows += 1;
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn bytes(&self) -> usize {
        self.halves.len() * 2
    }

    pub fn row(&self, index: usize) -> Vec<f32> {
        self.halves[index * DIMENSION..(index + 1) * DIMENSION]
            .iter()
            .map(|&bits| f16::from_bits(bits).to_f32())
            .collect()
    }

    /// The cosine of `query` with every row (all are L2-normalized): a matrix-vector product, in parallel over blocks
    /// of rows when there are many.
    pub fn scores(&self, query: &[f32]) -> Vec<f32> {
        assert_eq!(query.len(), DIMENSION);
        let mut scores = vec![0f32; self.rows];
        const BLOCK: usize = 1024;
        let kernel = Kernel::detect();
        if self.rows >= 16 * BLOCK {
            scores
                .par_chunks_mut(BLOCK)
                .enumerate()
                .for_each(|(block, out)| {
                    let first = block * BLOCK;
                    for (i, score) in out.iter_mut().enumerate() {
                        *score = kernel.dot(self.row_bits(first + i), query);
                    }
                });
        } else {
            for (i, score) in scores.iter_mut().enumerate() {
                *score = kernel.dot(self.row_bits(i), query);
            }
        }
        scores
    }

    fn row_bits(&self, index: usize) -> &[u16] {
        &self.halves[index * DIMENSION..(index + 1) * DIMENSION]
    }
}

/// The dot product of a float16 row with a float32 query: AVX2 + F16C + FMA where the CPU has them (every x86-64 CPU
/// since 2013), plain Rust elsewhere.
#[derive(Clone, Copy)]
enum Kernel {
    #[cfg(target_arch = "x86_64")]
    Avx2,
    Scalar,
}

impl Kernel {
    fn detect() -> Kernel {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("f16c")
            && std::arch::is_x86_feature_detected!("fma")
        {
            return Kernel::Avx2;
        }
        Kernel::Scalar
    }

    #[inline]
    fn dot(self, row: &[u16], query: &[f32]) -> f32 {
        match self {
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => unsafe { dot_avx2(row, query) },
            Kernel::Scalar => row
                .iter()
                .zip(query)
                .map(|(&bits, &q)| f16::from_bits(bits).to_f32() * q)
                .sum(),
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,f16c,fma")]
unsafe fn dot_avx2(row: &[u16], query: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    debug_assert_eq!(row.len(), DIMENSION);
    let (mut a, mut b, mut c, mut d) = (
        _mm256_setzero_ps(),
        _mm256_setzero_ps(),
        _mm256_setzero_ps(),
        _mm256_setzero_ps(),
    );
    let (r, q) = (row.as_ptr(), query.as_ptr());
    let mut i = 0;
    // Four accumulators: 32 values a step, so the FMAs don't wait on each other.
    while i + 32 <= DIMENSION {
        a = _mm256_fmadd_ps(
            _mm256_cvtph_ps(_mm_loadu_si128(r.add(i).cast())),
            _mm256_loadu_ps(q.add(i)),
            a,
        );
        b = _mm256_fmadd_ps(
            _mm256_cvtph_ps(_mm_loadu_si128(r.add(i + 8).cast())),
            _mm256_loadu_ps(q.add(i + 8)),
            b,
        );
        c = _mm256_fmadd_ps(
            _mm256_cvtph_ps(_mm_loadu_si128(r.add(i + 16).cast())),
            _mm256_loadu_ps(q.add(i + 16)),
            c,
        );
        d = _mm256_fmadd_ps(
            _mm256_cvtph_ps(_mm_loadu_si128(r.add(i + 24).cast())),
            _mm256_loadu_ps(q.add(i + 24)),
            d,
        );
        i += 32;
    }
    let sum = _mm256_add_ps(_mm256_add_ps(a, b), _mm256_add_ps(c, d));
    let high = _mm256_extractf128_ps(sum, 1);
    let quad = _mm_add_ps(_mm256_castps256_ps128(sum), high);
    let pair = _mm_add_ps(quad, _mm_movehl_ps(quad, quad));
    let single = _mm_add_ss(pair, _mm_shuffle_ps(pair, pair, 1));
    _mm_cvtss_f32(single)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: u64) -> Vec<f32> {
        let mut state = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut v: Vec<f32> = (0..DIMENSION)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 2001) as f32 / 1000.0 - 1.0
            })
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }

    #[test]
    fn blobs_round_trip() {
        let v = unit(1);
        let back = from_blob(&to_blob(&v)).unwrap();
        assert!(v.iter().zip(&back).all(|(a, b)| (a - b).abs() < 1e-3));
        assert!(from_blob(&[0u8; 10]).is_none());
    }

    #[test]
    fn simd_scores_match_plain_ones() {
        let mut matrix = Matrix::with_capacity(20_000);
        for seed in 0..20_000 {
            matrix.push(&unit(seed));
        }
        let query = unit(7);
        let scores = matrix.scores(&query);
        for row in [0, 7, 1023, 1024, 19_999] {
            let plain: f32 = matrix.row(row).iter().zip(&query).map(|(a, b)| a * b).sum();
            assert!(
                (scores[row] - plain).abs() < 1e-4,
                "row {row}: {} vs {plain}",
                scores[row]
            );
        }
        assert!((scores[7] - 1.0).abs() < 1e-2); // the query against itself
        assert_eq!(matrix.bytes(), 20_000 * DIMENSION * 2);
    }
}
