//! This file contains all of the logic necessary to implement possibly the
//! worst mutator of all time, there is no science here
//!
//! We get passed a corpus in Mutator, because we need access to other inputs,
//! Corpus should implement these two methods:
//! - num_inputs() -> Returns the number of inputs in the Corpus
//! - get_input() -> Returns a slice view of an input in the Corpus
//!
//! This is inspired by: https://github.com/gamozolabs/basic_mutator, which in
//! turn is inspired by Hongfuzz. We don't use any of the Hongfuzz derived code
//! in here, just trying to implement our own stuff that tries to mirror what
//! AFL++ does. Eventually we'll try to just use LibAFL's mutator?

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::corpus::Corpus;

// The maximum number of stacked mutations we can apply, I *think* this is what
// AFL++ does
const MAX_STACK: usize = 6;

// The % at which Magic Numbers and Splicing are considered as mutation types
const LONGSHOT_MUTATION_RATE: usize = 5;

// The % at which we generate an input from scratch instead of mutating corpus
const GEN_SCRATCH_RATE: usize = 5;

// Used as the number of bytes for byte-specific corruption routines
const MAX_BYTE_CORRUPTION: usize = 64;

// Used as the maximum block size for block-specific corruption routines
const MAX_BLOCK_CORRUPTION: usize = 512;

// Used as the maximum number of bits we can corrupt
const MAX_BIT_CORRUPTION: usize = 64;

// List of magic numbers I thought might be interesting, we mutate these in
// some ways as well as we perform byte translations
const MAGIC_NUMBERS: &[u64] = &[
    0,        // Hmmm
    u64::MAX, // All max values
    u32::MAX as u64,
    u16::MAX as u64,
    u8::MAX as u64,
    i64::MAX as u64,
    i32::MAX as u64,
    i16::MAX as u64,
    i8::MAX as u64,
    i64::MIN as u64, // All min values
    i32::MIN as u64,
    i16::MIN as u64,
    i8::MIN as u64,
    0b1 << 63, // Top bits set
    0b1 << 31,
    0b1 << 15,
    0b1 << 7,
    !(0b1 << 63), // All bits except top
    !(0b1 << 31) & 0xFFFFFFFF,
    !(0b1 << 15) & 0xFFFF,
    !(0b1 << 7) & 0xFF,
    2, // Po2
    4,
    8,
    16,
    32,
    64,
    128,
    256,
    512,
    1024,
    2048,
    4096,
    8192,
    16384,
];

// Mutation type list
const MUTATIONS: [MutationTypes; 12] = [
    MutationTypes::ByteInsert,
    MutationTypes::ByteOverwrite,
    MutationTypes::ByteDelete,
    MutationTypes::BlockInsert,
    MutationTypes::BlockOverwrite,
    MutationTypes::BlockDelete,
    MutationTypes::BitFlip,
    MutationTypes::Grow,
    MutationTypes::Truncate,
    MutationTypes::MagicByteInsert,
    MutationTypes::MagicByteOverwrite,
    MutationTypes::Splice,
];

// Helper function
fn generate_seed() -> usize {
    let mut hasher = DefaultHasher::new();

    let rdtsc = unsafe { core::arch::x86_64::_rdtsc() };
    rdtsc.hash(&mut hasher);

    // Combine all sources of entropy
    hasher.finish() as usize
}

// Some basic mutation types that AFL++ seems to do in Havoc mode
#[derive(Clone, Debug)]
pub enum MutationTypes {
    ByteInsert,
    ByteOverwrite,
    ByteDelete,
    BlockInsert,
    BlockOverwrite,
    BlockDelete,
    BitFlip,
    Grow,
    Truncate,
    MagicByteInsert,
    MagicByteOverwrite,
    Splice,
}

#[derive(Clone, Default)]
pub struct Mutator {
    pub rng: usize,
    pub input: Vec<u8>,
    pub max_size: usize,
    pub last_mutation: Vec<MutationTypes>,
}

impl Mutator {
    pub fn new(seed: Option<usize>, max_size: usize) -> Self {
        // If pRNG seed not provided, make our own
        let rng = if let Some(seed_val) = seed {
            seed_val
        } else {
            generate_seed()
        };

        Mutator {
            rng,
            input: Vec::with_capacity(max_size),
            max_size,
            last_mutation: Vec::with_capacity(MAX_STACK),
        }
    }

    pub fn reseed(&mut self) -> usize {
        self.rng = generate_seed();

        self.rng
    }

    #[inline]
    fn rand(&mut self) -> usize {
        // Save off current value
        let curr = self.rng;

        // Mutate current state with xorshift for next call
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 43;

        // Return saved off value
        curr
    }

    // Insert bytes into the input randomly
    fn byte_insert(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_INSERTS: usize = MAX_BYTE_CORRUPTION;

        // Determine the slack space we have
        let slack = self.max_size - self.input.len();

        // If we don't have any slack, return
        if slack == 0 {
            return;
        }

        // Determine the ceiling
        let ceiling = std::cmp::min(slack, MAX_INSERTS);

        // Pick number of bytes to insert, at least 1
        let insert_num = (self.rand() % ceiling) + 1;

        // Iterate through and apply insertions, duplicate idxs is ok
        for _ in 0..insert_num {
            // Pick an index
            let curr_idx = self.rand() % self.input.len();

            // Pick a byte to insert
            let byte = (self.rand() % 256) as u8;

            // Insert it
            self.input.insert(curr_idx, byte);
        }
    }

    // Overwrite bytes randomly
    fn byte_overwrite(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_OVERWRITES: usize = MAX_BYTE_CORRUPTION;

        // Determine how many bytes we can overwrite
        let ceiling = std::cmp::min(self.input.len(), MAX_OVERWRITES);

        // Pick a number of bytes to overwrite
        let overwrite_num = (self.rand() % ceiling) + 1;

        // Iterate through and apply overwrites
        for _ in 0..overwrite_num {
            // Pick an index
            let curr_idx = self.rand() % self.input.len();

            // Pick a byte to overwrite with
            let byte = (self.rand() % 256) as u8;

            // Overwrite it
            self.input[curr_idx] = byte;
        }
    }

    // Delete bytes randomly
    fn byte_delete(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_DELETES: usize = MAX_BYTE_CORRUPTION;

        // Determine how many bytes we can delete
        let ceiling = std::cmp::min(self.input.len() - 1, MAX_DELETES);

        // If the ceiling is 0, return
        if ceiling == 0 {
            return;
        }

        // Pick a number of bytes to delete
        let delete_num = (self.rand() % ceiling) + 1;

        // Iterate through and apply the deletes
        for _ in 0..delete_num {
            // Pick an index
            let curr_idx = self.rand() % self.input.len();

            // Remove it
            self.input.remove(curr_idx);
        }
    }

    // Grab a block from the input, and insert it randomly somewhere
    fn block_insert(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_BLOCK_SIZE: usize = MAX_BLOCK_CORRUPTION;
        let mut block = [0u8; MAX_BLOCK_SIZE];

        // Determine the slack space in the input we have since we're growing
        let slack = self.max_size - self.input.len();

        // If we don't have any slack, return
        if slack == 0 {
            return;
        }

        // Determine a ceiling
        let mut ceiling = std::cmp::min(slack, MAX_BLOCK_SIZE);

        // If the ceiling is larger than the input, adjust it
        if ceiling > self.input.len() {
            ceiling = self.input.len();
        }

        // Determine a block size
        let block_size = (self.rand() % ceiling) + 1;

        // Determine the end range we can start from for the block
        let max_start = self.input.len() - block_size;

        // Determine where to start reading the block
        let block_start = self.rand() % (max_start + 1);

        // Copy the block into the block array
        block[..block_size].copy_from_slice(&self.input[block_start..block_start + block_size]);

        // Determine where to insert the block
        let block_insert = self.rand() % self.input.len();

        // Use insert calls (slow, but readable and who cares?)
        for (i, &byte) in block[..block_size].iter().enumerate() {
            self.input.insert(block_insert + i, byte);
        }
    }

    // Grab a block from the input and overwrite the contents somewhere with it
    fn block_overwrite(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_BLOCK_SIZE: usize = MAX_BLOCK_CORRUPTION;
        let mut block = [0u8; MAX_BLOCK_SIZE];

        // Determine a ceiling of block size
        let ceiling = std::cmp::min(self.input.len(), MAX_BLOCK_SIZE);

        // Pick a block size
        let block_size = (self.rand() % ceiling) + 1;

        // Determine the end range we can start from for the block reading, but
        // also this is the block writing start as well
        let max_start = self.input.len() - block_size;

        // Determine where to start reading the block
        let block_start = self.rand() % (max_start + 1);

        // Copy the block into the block array
        block[..block_size].copy_from_slice(&self.input[block_start..block_start + block_size]);

        // Determine where to start overwriting
        let overwrite_start = self.rand() % (max_start + 1);

        // Overwrite those bytes
        self.input[overwrite_start..overwrite_start + block_size]
            .copy_from_slice(&block[..block_size]);
    }

    // Remove a random block from the input
    fn block_delete(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_BLOCK_SIZE: usize = MAX_BLOCK_CORRUPTION;

        // Determine how much we can delete
        let ceiling = std::cmp::min(self.input.len() - 1, MAX_BLOCK_SIZE);

        // If we have a ceiling of 0, just return
        if ceiling == 0 {
            return;
        }

        // Pick a block size for deletion
        let block_size = (self.rand() % ceiling) + 1;

        // Determine the end range to start deleting from
        let max_start = self.input.len() - block_size;

        // Pick a place to start deleting from
        let block_start = self.rand() % (max_start + 1);

        // Delete that block
        self.input.drain(block_start..block_start + block_size);
    }

    // Generate a random input
    fn generate_random_input(&mut self) {
        // Pick a size for the input
        let input_size = (self.rand() % self.max_size) + 1;

        // Re-size the input vector
        self.input.resize(input_size, 0);

        // Fill in the data randomly
        for i in 0..input_size {
            self.input[i] = (self.rand() % 256) as u8;
        }
    }

    // Randomly flip bits in the input
    fn bit_flip(&mut self) {
        // Determine the number of bits in the input
        let num_bits = self.input.len() * 8;

        // Determine the ceiling of what we can flip
        let ceiling = std::cmp::min(num_bits, MAX_BIT_CORRUPTION);

        // Determine the number of bits to flip (at least 1)
        let num_flips = (self.rand() % ceiling) + 1;

        // Go through and flip bits
        for _ in 0..num_flips {
            // Choose a random bit to flip
            let bit_position = self.rand() % num_bits;

            // Calculate which byte this bit is in
            let byte_index = bit_position / 8;

            // Calculate which bit within the byte to flip
            let bit_index = bit_position % 8;

            // Flip the bit
            self.input[byte_index] ^= 1 << bit_index;
        }
    }

    // Randomly insert random byte block into input
    fn grow(&mut self) {
        // Determine maximum size to grow
        let slack = self.max_size - self.input.len();
        if slack == 0 {
            return;
        }

        // Pick size of block
        let size = (self.rand() % slack) + 1;

        // Pick an index to add to
        let idx = self.rand() % self.input.len();

        // Pick byte to place in there
        let byte = (self.rand() % 256) as u8;

        // Insert there
        for _ in 0..size {
            self.input.insert(idx, byte);
        }
    }

    // Randomly truncate the input, always leave at least 1 byte
    fn truncate(&mut self) {
        // Determine how much we can shrink
        let slack = self.input.len() - 1;
        if slack == 0 {
            return;
        }

        // Pick an index to truncate at, can't be zero
        let idx = (self.rand() % slack) + 1;

        // Truncate
        self.input.truncate(idx);
    }

    // Randomly mutate a magic number
    fn mutate_magic(&mut self, magic: u64) -> Vec<u8> {
        // Mutate the magic value
        let magic = match self.rand() % 14 {
            0 => magic,
            1 => magic & 0xFF,
            2 => magic & 0xFFFF,
            3 => magic & 0xFFFFFFFF,
            4 => magic - 1,
            5 => magic + 1,
            6 => !magic,                 // Bitwise NOT
            7 => magic << 1,             // Left shift by 1
            8 => magic >> 1,             // Right shift by 1
            9 => magic.rotate_left(8),   // Rotate left by 8 bits
            10 => magic.rotate_right(8), // Rotate right by 8 bits
            11 => magic ^ 0xFFFFFFFF,    // XOR with all 1s (32-bit)
            12 => magic.swap_bytes(),    // Swap byte order
            13 => {
                // Flip a random bit
                let bit = self.rand() % 64;
                magic ^ (1 << bit)
            }
            _ => unreachable!(),
        };

        // Convert to bytes
        let magic_bytes = magic.to_ne_bytes();

        // Randomly truncate bytes
        match self.rand() % 15 {
            0 => magic_bytes.to_vec(),       // All 8 bytes (u64)
            1 => magic_bytes[0..4].to_vec(), // First 4 bytes (u32)
            2 => magic_bytes[4..8].to_vec(), // Last 4 bytes (u32)
            3 => magic_bytes[0..2].to_vec(), // First 2 bytes (u16)
            4 => magic_bytes[2..4].to_vec(), // Second 2 bytes (u16)
            5 => magic_bytes[4..6].to_vec(), // Third 2 bytes (u16)
            6 => magic_bytes[6..8].to_vec(), // Last 2 bytes (u16)
            7 => vec![magic_bytes[0]],       // 1st byte (u8)
            8 => vec![magic_bytes[1]],       // 2nd byte (u8)
            9 => vec![magic_bytes[2]],       // 3rd byte (u8)
            10 => vec![magic_bytes[3]],      // 4th byte (u8)
            11 => vec![magic_bytes[4]],      // 5th byte (u8)
            12 => vec![magic_bytes[5]],      // 6th byte (u8)
            13 => vec![magic_bytes[6]],      // 7th byte (u8)
            14 => vec![magic_bytes[7]],      // 8th byte (u8)
            _ => unreachable!(),
        }
    }

    // Randomly insert magic bytes into the input
    fn magic_byte_insert(&mut self) {
        // Defaults to global max, but can be hand tuned
        const MAX_INSERTS: usize = MAX_BYTE_CORRUPTION;

        // Determine the slack space we have
        let slack = self.max_size - self.input.len();

        // If we don't have any slack space, return
        if slack == 0 {
            return;
        }

        // Determine the ceiling
        let ceiling = std::cmp::min(slack, MAX_INSERTS);

        // Pick number of bytes to insert, at least 1
        let insert_num = (self.rand() % ceiling) + 1;

        // Divide that by 8 to determine how many u64s will fit
        let num_u64 = insert_num / 8;

        // Insert up to num_u64 u64 values, likely much smaller
        for _ in 0..num_u64 {
            // Pick an index to insert at
            let idx = self.rand() % self.input.len();

            // Pick a magic value
            let magic = MAGIC_NUMBERS[self.rand() % MAGIC_NUMBERS.len()];

            // Randomly corrupt the magic number
            let magic_bytes = if self.rand() % 2 == 0 {
                self.mutate_magic(magic)
            } else {
                magic.to_ne_bytes().to_vec()
            };

            // Insert magic bytes
            for (i, &byte) in magic_bytes.iter().enumerate() {
                self.input.insert(idx + i, byte);
            }
        }
    }

    // Randomly overwrite bytes in the input with magic bytes
    fn magic_byte_overwrite(&mut self) {
        // If the input isn't at least 8 bytes, just NOP
        if self.input.len() < 8 {
            return;
        }

        // Defaults to global max, but can be hand tuned
        const MAX_OVERWRITES: usize = MAX_BYTE_CORRUPTION;

        // Determine how many bytes we can overwrite
        let ceiling = std::cmp::min(self.input.len(), MAX_OVERWRITES);

        // Pick a number of bytes to overwrite
        let overwrite_num = (self.rand() % ceiling) + 1;

        // Divide that number by 8 to determine how many u64s will fit
        let num_u64 = overwrite_num / 8;

        // Make sure we don't go out of bounds
        let max_overwrite = self.input.len() - 8;

        // Overwrite up to num_u64 u64 values
        for _ in 0..num_u64 {
            // Pick an index to overwrite at
            let idx = self.rand() % (max_overwrite + 1);

            // Pick a magic value
            let magic = MAGIC_NUMBERS[self.rand() % MAGIC_NUMBERS.len()];

            // Randomly corrupt the magic number
            let magic_bytes = if self.rand() % 2 == 0 {
                self.mutate_magic(magic)
            } else {
                magic.to_ne_bytes().to_vec()
            };

            // Overwrite with magic bytes
            for (i, &byte) in magic_bytes.iter().enumerate() {
                self.input[idx + i] = byte;
            }
        }
    }

    // Splice two inputs together
    fn splice(&mut self, corpus: &Corpus) {
        // Take a block of the current input
        let old_block_start = self.rand() % self.input.len();

        // Pick a length for the block
        let old_block_len = self.rand() % (self.input.len() - old_block_start) + 1;

        // Pick a new input index
        let new_idx = self.rand() % corpus.num_inputs();

        // Get reference to new input
        let Some(new_input) = corpus.get_input(new_idx) else {
            return; // No inputs in corpus?
        };

        // Determine the slack space left
        let slack = self.max_size - old_block_len;

        // If there's no slack, we can return early
        if slack == 0 {
            return;
        }

        // Pick a place in the new input to read a block from
        let new_block_start = self.rand() % new_input.len();

        // Pick a length ceiling of the new block, guaranteed to be at least 1
        let new_ceiling = std::cmp::min(new_input.len() - new_block_start, slack);

        // Pick a length
        let new_block_len = (self.rand() % new_ceiling) + 1;

        // Determine total length we'll have
        let total_len = old_block_len + new_block_len;

        // Adjust input buffer if necessary
        if total_len > self.input.len() {
            self.input.resize(total_len, 0);
        }

        // Copy with memmove because of overlap potential
        self.input
            .copy_within(old_block_start..old_block_start + old_block_len, 0);

        // Then, copy the new block right after the old block
        let new_block = &new_input[new_block_start..new_block_start + new_block_len];
        self.input[old_block_len..total_len].copy_from_slice(new_block);

        // Adjust input buffer length if necessary
        if total_len < self.input.len() {
            self.input.truncate(total_len);
        }
    }

    pub fn mutate_input(&mut self, corpus: &Corpus) {
        // Clear current input
        self.input.clear();
        self.last_mutation.clear();

        // Get the number of inputs to choose from
        let num_inputs = corpus.num_inputs();

        // n% of the time, just generate a new input from scratch
        let gen = self.rand() % 100;

        // If we don't have any inputs to choose from, create a random one
        if num_inputs == 0 || gen < GEN_SCRATCH_RATE {
            self.generate_random_input();
            return;
        }

        // Pick an input from the corpus to use
        let idx = self.rand() % num_inputs;

        // Get the input
        let chosen = corpus.get_input(idx).unwrap();

        // Copy the input over
        self.input.extend_from_slice(chosen);

        // We have an input, pick a number of rounds of mutation
        let rounds = (self.rand() % MAX_STACK) + 1;

        // Apply mutations for number of rounds
        for _ in 0..rounds {
            // Determine the pool of candidates, we don't want to frequently
            // use longshot strategies
            let longshot = self.rand() % 100;

            // If we're within the longshot range, add them to the possible
            let pool = if longshot <= LONGSHOT_MUTATION_RATE {
                MUTATIONS.len()
            } else {
                MUTATIONS.len() - 3
            };

            // Pick mutation type
            let mutation_idx = self.rand() % pool;

            // Match on the mutation and apply it
            match MUTATIONS[mutation_idx] {
                MutationTypes::ByteInsert => {
                    self.byte_insert();
                    self.last_mutation.push(MutationTypes::ByteInsert);
                }
                MutationTypes::ByteOverwrite => {
                    self.byte_overwrite();
                    self.last_mutation.push(MutationTypes::ByteOverwrite);
                }
                MutationTypes::ByteDelete => {
                    self.byte_delete();
                    self.last_mutation.push(MutationTypes::ByteDelete);
                }
                MutationTypes::BlockInsert => {
                    self.block_insert();
                    self.last_mutation.push(MutationTypes::BlockInsert);
                }
                MutationTypes::BlockOverwrite => {
                    self.block_overwrite();
                    self.last_mutation.push(MutationTypes::BlockOverwrite);
                }
                MutationTypes::BlockDelete => {
                    self.block_delete();
                    self.last_mutation.push(MutationTypes::BlockDelete);
                }
                MutationTypes::BitFlip => {
                    self.bit_flip();
                    self.last_mutation.push(MutationTypes::BitFlip);
                }
                MutationTypes::Grow => {
                    self.grow();
                    self.last_mutation.push(MutationTypes::Grow);
                }
                MutationTypes::Truncate => {
                    self.truncate();
                    self.last_mutation.push(MutationTypes::Truncate);
                }
                MutationTypes::MagicByteInsert => {
                    self.magic_byte_insert();
                    self.last_mutation.push(MutationTypes::MagicByteInsert);
                }
                MutationTypes::MagicByteOverwrite => {
                    self.magic_byte_overwrite();
                    self.last_mutation.push(MutationTypes::MagicByteOverwrite);
                }
                MutationTypes::Splice => {
                    self.splice(corpus);
                    self.last_mutation.push(MutationTypes::Splice);
                }
            }
        }

        // This isn't prod
        assert!(!self.input.is_empty());
        assert!(self.input.len() <= self.max_size);
    }

    // Take a slice from someone and copy into our input buffer, used by
    // Redqueen right now to test traces
    pub fn memcpy_input(&mut self, slice: &[u8]) {
        // Clear the current input
        self.input.clear();

        // Copy the passed in buffer
        self.input.extend_from_slice(slice);
    }
}
