/// The Xoroshiro seed Minecraft derives from a string: the first and second 8 bytes of its MD5
/// hash, read as big-endian integers.
pub fn xoroshiro_seed(seed: &str) -> (u64, u64) {
    let hash = md5::compute(seed);
    let l = u64::from_be_bytes([
        hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7],
    ]);
    let m = u64::from_be_bytes([
        hash[8], hash[9], hash[10], hash[11], hash[12], hash[13], hash[14], hash[15],
    ]);
    (l, m)
}
