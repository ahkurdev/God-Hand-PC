//! Static recompilation of God Hand's MIPS R5900 code to x86-64.
//!
//! The port is native rather than emulated: MIPS instructions are translated
//! once into host machine code, which then runs directly on the CPU. This
//! module holds the decoder, the lifter, the memory space the lifted code
//! addresses, and the execution harness that proves the emitted code runs.

pub mod decode;
pub mod exec;
pub mod lifter;
pub mod memory;

pub use decode::{classify, Classified, Insn, Op, SpecialFn};
pub use exec::{new_regs, ExecutableCode, RegFile};
pub use lifter::{CodeBuf, Lifter, LiftResult, REG_FILE_SIZE};
pub use memory::{Memory, MEMORY_SIZE};
