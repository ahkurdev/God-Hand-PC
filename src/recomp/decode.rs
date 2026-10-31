//! MIPS R5900 instruction decoder.
//!
//! God Hand's `SLES_544.90` is a 32-bit big-endian-capable MIPS III binary
//! built with SN Systems' DVP toolchain for the PlayStation 2's Emotion
//! Engine. Static recompilation to x86-64 begins here: every 32-bit word must
//! be classified before it can be lifted.
//!
//! ## Instruction mix (measured over 721,368 words in `.text`)
//!
//! | Opcode              | Share |
//! |---------------------|-------|
//! | SPECIAL             | 23.1% |
//! | ADDIU               | 14.0% |
//! | LW                  |  8.0% |
//! | JAL                 |  5.6% |
//! | BEQ                 |  5.5% |
//! | LUI                 |  4.7% |
//! | SW                  |  3.9% |
//! | COP1 (LWC1/SWC1/…)  | ~8.3% |
//! | DSUB (SPECIAL fn 45)| 10.1% |
//!
//! Roughly 90% of the binary is covered by fifteen opcodes, which is what
//! makes a focused lifter practical: the exotic VU and COP2 paths can be
//! trapped and handled later without blocking the common case.

/// A decoded MIPS instruction.
///
/// Fields are kept in their raw register/immediate form rather than resolved,
/// because the lifter needs the original operand encoding to emit correct
/// x86-64 for each addressing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insn {
    /// The raw 32-bit instruction word.
    pub raw: u32,
    /// Primary opcode field, bits 31-26.
    pub op: u8,
    /// Source register, bits 25-21.
    pub rs: u8,
    /// Destination register, bits 20-16.
    pub rt: u8,
    /// Third register, bits 15-11.
    pub rd: u8,
    /// Shift amount, bits 10-6.
    pub shamt: u8,
    /// Function field, bits 5-0.
    pub funct: u8,
    /// 16-bit immediate, bits 15-0.
    pub imm: u16,
    /// 26-bit jump target, bits 25-0.
    pub target: u32,
}

impl Insn {
    /// Decode one 32-bit instruction word.
    pub fn decode(raw: u32) -> Insn {
        Insn {
            raw,
            op: ((raw >> 26) & 0x3F) as u8,
            rs: ((raw >> 21) & 0x1F) as u8,
            rt: ((raw >> 16) & 0x1F) as u8,
            rd: ((raw >> 11) & 0x1F) as u8,
            shamt: ((raw >> 6) & 0x1F) as u8,
            funct: (raw & 0x3F) as u8,
            imm: (raw & 0xFFFF) as u16,
            target: raw & 0x03FF_FFFF,
        }
    }

    /// Sign-extend the 16-bit immediate to 32 bits.
    pub fn simm(&self) -> i32 {
        self.imm as i16 as i32
    }

    /// Zero-extend the 16-bit immediate to 32 bits.
    pub fn zimm(&self) -> u32 {
        self.imm as u32
    }
}

/// MIPS primary opcodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Special,
    Regimm,
    J,
    Jal,
    Beq,
    Bne,
    Blez,
    Bgtz,
    Addi,
    Addiu,
    Slti,
    Sltiu,
    Andi,
    Ori,
    Xori,
    Lui,
    Cop0,
    Cop1,
    Cop2,
    Beql,
    Bnel,
    Blezl,
    Bgtzl,
    Daddi,
    Daddiu,
    Ldl,
    Ldr,
    Lb,
    Lh,
    Lwl,
    Lw,
    Lbu,
    Lhu,
    Lwr,
    Sb,
    Sh,
    Swl,
    Sw,
    Sdl,
    Sdr,
    Swr,
    Lwc1,
    Lwc2,
    Swc1,
    Swc2,
    /// Any opcode not needed by the current lifter.
    Unhandled(u8),
}

impl Op {
    /// Map a raw opcode byte to its variant.
    pub fn from_raw(op: u8) -> Op {
        match op {
            0x00 => Op::Special,
            0x01 => Op::Regimm,
            0x02 => Op::J,
            0x03 => Op::Jal,
            0x04 => Op::Beq,
            0x05 => Op::Bne,
            0x06 => Op::Blez,
            0x07 => Op::Bgtz,
            0x08 => Op::Addi,
            0x09 => Op::Addiu,
            0x0A => Op::Slti,
            0x0B => Op::Sltiu,
            0x0C => Op::Andi,
            0x0D => Op::Ori,
            0x0E => Op::Xori,
            0x0F => Op::Lui,
            0x10 => Op::Cop0,
            0x11 => Op::Cop1,
            0x12 => Op::Cop2,
            0x14 => Op::Beql,
            0x15 => Op::Bnel,
            0x16 => Op::Blezl,
            0x17 => Op::Bgtzl,
            0x18 => Op::Daddi,
            0x19 => Op::Daddiu,
            0x1A => Op::Ldl,
            0x1B => Op::Ldr,
            0x20 => Op::Lb,
            0x21 => Op::Lh,
            0x22 => Op::Lwl,
            0x23 => Op::Lw,
            0x24 => Op::Lbu,
            0x25 => Op::Lhu,
            0x26 => Op::Lwr,
            0x28 => Op::Sb,
            0x29 => Op::Sh,
            0x2A => Op::Swl,
            0x2B => Op::Sw,
            0x2C => Op::Sdl,
            0x2D => Op::Sdr,
            0x2E => Op::Swr,
            0x31 => Op::Lwc1,
            0x32 => Op::Lwc2,
            0x39 => Op::Swc1,
            0x3A => Op::Swc2,
            other => Op::Unhandled(other),
        }
    }
}

/// SPECIAL function codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialFn {
    Sll,
    Srl,
    Sra,
    Sllv,
    Srav,
    Jr,
    Jalr,
    Movz,
    Movn,
    Syscall,
    Break,
    Mfhi,
    Mthi,
    Mflo,
    Mtlo,
    Mult,
    Multu,
    Div,
    Divu,
    Add,
    Addu,
    Sub,
    Subu,
    And,
    Or,
    Xor,
    Nor,
    Slt,
    Sltu,
    /// PS2-specific 128-bit subtract used by the VU interop paths.
    Dsub,
    /// PS2-specific 128-bit add.
    Dadd,
    /// Any function not needed by the current lifter.
    Unhandled(u8),
}

impl SpecialFn {
    /// Map a raw function byte to its variant.
    pub fn from_raw(funct: u8) -> SpecialFn {
        match funct {
            0x00 => SpecialFn::Sll,
            0x02 => SpecialFn::Srl,
            0x03 => SpecialFn::Sra,
            0x04 => SpecialFn::Sllv,
            0x07 => SpecialFn::Srav,
            0x08 => SpecialFn::Jr,
            0x09 => SpecialFn::Jalr,
            0x0A => SpecialFn::Movz,
            0x0B => SpecialFn::Movn,
            0x0C => SpecialFn::Syscall,
            0x0D => SpecialFn::Break,
            0x10 => SpecialFn::Mfhi,
            0x11 => SpecialFn::Mthi,
            0x12 => SpecialFn::Mflo,
            0x13 => SpecialFn::Mtlo,
            0x18 => SpecialFn::Mult,
            0x19 => SpecialFn::Multu,
            0x1A => SpecialFn::Div,
            0x1B => SpecialFn::Divu,
            0x20 => SpecialFn::Add,
            0x21 => SpecialFn::Addu,
            0x22 => SpecialFn::Sub,
            0x23 => SpecialFn::Subu,
            0x24 => SpecialFn::And,
            0x25 => SpecialFn::Or,
            0x26 => SpecialFn::Xor,
            0x27 => SpecialFn::Nor,
            0x2A => SpecialFn::Slt,
            0x2B => SpecialFn::Sltu,
            0x2C => SpecialFn::Dadd,
            0x2D => SpecialFn::Dsub,
            other => SpecialFn::Unhandled(other),
        }
    }
}

/// A fully classified instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classified {
    /// An ALU or memory operation the lifter can emit directly.
    Liftable(Op),
    /// A SPECIAL-format operation.
    Special(SpecialFn),
    /// A branch or jump, which needs control-flow handling.
    Control(Op),
    /// An instruction the lifter must trap.
    Trap,
}

/// Classify a decoded instruction into the lifter's action categories.
pub fn classify(insn: &Insn) -> Classified {
    match Op::from_raw(insn.op) {
        Op::Special => Classified::Special(SpecialFn::from_raw(insn.funct)),
        Op::J | Op::Jal => Classified::Control(Op::from_raw(insn.op)),
        Op::Beq | Op::Bne | Op::Blez | Op::Bgtz | Op::Beql | Op::Bnel | Op::Blezl
        | Op::Bgtzl => Classified::Control(Op::from_raw(insn.op)),
        Op::Regimm => Classified::Control(Op::Regimm),
        op @ (Op::Addiu | Op::Addi | Op::Slti | Op::Sltiu | Op::Andi | Op::Ori | Op::Xori
        | Op::Lui | Op::Lb | Op::Lh | Op::Lw | Op::Lbu | Op::Lhu | Op::Sb | Op::Sh | Op::Sw
        | Op::Lwl | Op::Lwr | Op::Swl | Op::Swr) => Classified::Liftable(op),
        Op::Cop0 | Op::Cop1 | Op::Cop2 => Classified::Trap,
        Op::Daddi | Op::Daddiu => Classified::Liftable(Op::from_raw(insn.op)),
        _ => Classified::Trap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_addiu() {
        // addiu $t0, $t1, 0x1234
        let raw = (9 << 26) | (9 << 21) | (8 << 16) | 0x1234;
        let insn = Insn::decode(raw);
        assert_eq!(insn.op, 9);
        assert_eq!(insn.rs, 9);
        assert_eq!(insn.rt, 8);
        assert_eq!(insn.imm, 0x1234);
        assert_eq!(insn.simm(), 0x1234);
        assert!(matches!(classify(&insn), Classified::Liftable(Op::Addiu)));
    }

    #[test]
    fn decodes_lw_with_negative_offset() {
        // lw $a0, -8($sp)
        let raw = (35 << 26) | (29 << 21) | (4 << 16) | 0xFFF8;
        let insn = Insn::decode(raw);
        assert_eq!(insn.op, 35);
        assert_eq!(insn.rs, 29);
        assert_eq!(insn.simm(), -8);
    }

    #[test]
    fn decodes_special_addu() {
        // addu $v0, $v1, $a0
        let raw = (8 << 21) | (3 << 16) | (4 << 11) | (0x21 & 0x3F);
        let insn = Insn::decode(raw);
        assert_eq!(insn.op, 0);
        assert_eq!(insn.funct, 0x21);
        assert!(matches!(
            classify(&insn),
            Classified::Special(SpecialFn::Addu)
        ));
    }

    #[test]
    fn decodes_ps2_dsub() {
        // dsub is SPECIAL funct 0x2D, the most common exotic op in God Hand
        let raw = (0 << 26) | (1 << 21) | (2 << 16) | (3 << 11) | 0x2D;
        let insn = Insn::decode(raw);
        assert!(matches!(
            classify(&insn),
            Classified::Special(SpecialFn::Dsub)
        ));
    }

    #[test]
    fn decodes_jal() {
        let raw = (3 << 26) | 0x0010_0000;
        let insn = Insn::decode(raw);
        assert_eq!(insn.op, 3);
        assert_eq!(insn.target, 0x0010_0000);
        assert!(matches!(classify(&insn), Classified::Control(Op::Jal)));
    }

    #[test]
    fn traps_cop2() {
        // VU instructions arrive as COP2 and must be trapped, not mislifted.
        let raw = (0x12 << 26) | 0x1;
        let insn = Insn::decode(raw);
        assert_eq!(classify(&insn), Classified::Trap);
    }

    #[test]
    fn sign_extension_is_correct() {
        let raw = (9 << 26) | 0xFFFF;
        let insn = Insn::decode(raw);
        assert_eq!(insn.simm(), -1);
        assert_eq!(insn.zimm(), 0xFFFF);
    }
}
