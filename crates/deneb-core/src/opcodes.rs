pub const COMMAND_GROUP: i64 = 500;
pub const FIRST_OFFICIAL_EXTENSION: i64 = 501;
pub const FIRST_THIRD_PARTY_EXTENSION: i64 = 10_000;

pub const NATIVE_OPCODES: &[i64] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
    33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
    62, 63, 64, 65, 66, 67,
];

pub fn is_native(opcode: i64) -> bool {
    NATIVE_OPCODES.binary_search(&opcode).is_ok() || opcode == COMMAND_GROUP
}

pub fn is_retired(opcode: i64) -> bool {
    matches!(opcode, 8 | 22)
}

pub fn is_reserved(opcode: i64) -> bool {
    (0..=100).contains(&opcode) || opcode == COMMAND_GROUP
}
