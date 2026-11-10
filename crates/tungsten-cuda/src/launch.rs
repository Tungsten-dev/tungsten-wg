/*
Launch shape of an instance's kernel: a 1D grid of `ceil(N / B)` blocks, so no lanes are padded
away on narrow dimensions; the wrapper decodes the linear id back into `(x, y, z)`. Deterministic,
so the device side (`__launch_bounds__`) and the host side (`<<<blocks, threads>>>`) compute the
same shape. Keep this and the kernel body signature stable: wave fusion (see `README.md`) builds on
both.
*/

/// Block count and threads per block of one 1D launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchShape {
    pub blocks: u32,
    pub threads: u32,
}

impl LaunchShape {
    pub fn threads_per_block(&self) -> u32 {
        self.threads
    }
}

/// The shape for `dimensions` with at most `block_size` threads per block: one thread per element,
/// threads past the element count return.
pub fn launch_shape(dimensions: (i32, i32, i32), block_size: u32) -> LaunchShape {
    let (dx, dy, dz) = dimensions;
    assert!(dx > 0 && dy > 0 && dz > 0, "instance dimensions must be positive, got {:?}", dimensions);
    let elements = dx as i64 * dy as i64 * dz as i64;
    assert!(elements <= i32::MAX as i64, "instance {:?} has more than i32::MAX elements", dimensions);
    let elements = elements as u32;
    let threads = elements.min(block_size);
    LaunchShape { blocks: elements.div_ceil(threads), threads }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(launch_shape((16, 16, 16), 128), LaunchShape { blocks: 32, threads: 128 });
        assert_eq!(launch_shape((5, 49, 5), 128), LaunchShape { blocks: 10, threads: 128 });
        assert_eq!(launch_shape((5, 1, 5), 128), LaunchShape { blocks: 1, threads: 25 });
        assert_eq!(launch_shape((1, 1, 1), 128), LaunchShape { blocks: 1, threads: 1 });
        assert_eq!(launch_shape((1000, 3, 2), 256), LaunchShape { blocks: 24, threads: 256 });
    }
}
