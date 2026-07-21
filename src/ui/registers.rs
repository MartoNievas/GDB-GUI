//! Clasificación de registros por nombre, independiente de la arquitectura.
//!
//! GDB devuelve los registros de la arquitectura activa (x86-64, x86-32, ARM64,
//! ARM32, RISC-V) mezclados en una sola lista. Clasificarlos por nombre es
//! inherentemente ambiguo (`s0` es entero en RISC-V pero coma flotante en
//! AArch64; `x0` es de propósito general en ARM64/RISC-V…), así que en vez de
//! predicados sueltos que pueden solaparse usamos un único `classify` con una
//! **precedencia explícita**: cada registro cae en exactamente una categoría.

/// Categoría a la que pertenece un registro.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegClass {
    /// Propósito general (rax, x0, a0…).
    General,
    /// Puntero de instrucción y flags (rip, pc, eflags, cpsr…).
    Control,
    /// Registros de segmento x86 (cs, ds, ss…).
    Segment,
    /// Vectoriales / coma flotante (xmm, ymm, st, v0…, f0…).
    Simd,
    /// Cualquier otra cosa (fs_base, ftag, registros sin nombre…).
    Other,
}

/// Clasifica un registro por su nombre. La precedencia resuelve las
/// ambigüedades: control y segmento (nombres específicos) van primero;
/// propósito general antes que SIMD para que los `s0`–`s11` de RISC-V no se
/// confundan con los escalares de coma flotante de AArch64.
pub fn classify(name: &str) -> RegClass {
    let n = name.to_ascii_lowercase();
    let n = n.as_str();

    if is_control(n) {
        RegClass::Control
    } else if is_segment(n) {
        RegClass::Segment
    } else if is_general(n) {
        RegClass::General
    } else if is_simd(n) {
        RegClass::Simd
    } else {
        RegClass::Other
    }
}

/// En x86-64, `eax`/`ebx`/… son la mitad baja de `rax`/`rbx`/…; conviene
/// ocultarlas cuando el registro de 64 bits está presente.
pub fn is_x86_32_gp(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "eax" | "ebx" | "ecx" | "edx" | "esi" | "edi" | "ebp" | "esp" | "eip"
    )
}

/// Orden convencional dentro del grupo de propósito general (rax, rbx, …, rip).
/// Los nombres desconocidos van al final conservando el orden de GDB.
pub fn display_order(name: &str) -> u32 {
    match name.to_ascii_lowercase().as_str() {
        "rax" | "eax" => 0,
        "rbx" | "ebx" => 1,
        "rcx" | "ecx" => 2,
        "rdx" | "edx" => 3,
        "rsi" | "esi" => 4,
        "rdi" | "edi" => 5,
        "rbp" | "ebp" => 6,
        "rsp" | "esp" => 7,
        "r8" => 8,
        "r9" => 9,
        "r10" => 10,
        "r11" => 11,
        "r12" => 12,
        "r13" => 13,
        "r14" => 14,
        "r15" => 15,
        _ => u32::MAX,
    }
}

// ─── Predicados internos (operan sobre un nombre ya en minúsculas) ─────────────

fn is_control(n: &str) -> bool {
    matches!(
        n,
        // x86
        "rip" | "eip" | "rflags" | "eflags" | "flags"
        | "mxcsr" | "fctrl" | "fstat"
        // ARM
        | "pc" | "cpsr" | "spsr" | "fpsr" | "fpcr"
        // RISC-V
        | "fcsr"
    )
}

fn is_segment(n: &str) -> bool {
    matches!(n, "cs" | "ds" | "ss" | "es" | "fs" | "gs")
}

fn is_general(n: &str) -> bool {
    matches!(
        n,
        // x86-64
        "rax" | "rbx" | "rcx" | "rdx" | "rsi" | "rdi" | "rbp" | "rsp"
        | "r8" | "r9" | "r10" | "r11" | "r12" | "r13" | "r14" | "r15"
        // x86-32
        | "eax" | "ebx" | "ecx" | "edx" | "esi" | "edi" | "ebp" | "esp"
        // ARM64: x0-x30, w0-w30, sp, zr
        | "x0" | "x1" | "x2" | "x3" | "x4" | "x5" | "x6" | "x7"
        | "x8" | "x9" | "x10" | "x11" | "x12" | "x13" | "x14" | "x15"
        | "x16" | "x17" | "x18" | "x19" | "x20" | "x21" | "x22" | "x23"
        | "x24" | "x25" | "x26" | "x27" | "x28" | "x29" | "x30" | "x31"
        | "w0" | "w1" | "w2" | "w3" | "w4" | "w5" | "w6" | "w7"
        | "w8" | "w9" | "w10" | "w11" | "w12" | "w13" | "w14" | "w15"
        | "w16" | "w17" | "w18" | "w19" | "w20" | "w21" | "w22" | "w23"
        | "w24" | "w25" | "w26" | "w27" | "w28" | "w29" | "w30"
        | "xzr" | "wzr" | "sp"
        // ARM32: r0-r14, ip (r12), lr (r14)
        | "r0" | "r1" | "r2" | "r3" | "r4" | "r5" | "r6" | "r7"
        | "ip" | "lr"
        // RISC-V: nombres ABI enteros
        | "zero" | "ra" | "gp" | "tp" | "fp"
        | "a0" | "a1" | "a2" | "a3" | "a4" | "a5" | "a6" | "a7"
        | "s0" | "s1" | "s2" | "s3" | "s4" | "s5"
        | "s6" | "s7" | "s8" | "s9" | "s10" | "s11"
        | "t0" | "t1" | "t2" | "t3" | "t4" | "t5" | "t6"
    )
}

fn is_simd(n: &str) -> bool {
    // Prefijos vectoriales con índice numérico: xmm0-31, ymm0-31, zmm0-31.
    for p in ["xmm", "ymm", "zmm"] {
        if let Some(rest) = n.strip_prefix(p) {
            if rest.parse::<u32>().is_ok() {
                return true;
            }
        }
    }
    // Vistas de una letra que NO colisionan con propósito general:
    // v (vector AArch64), q (quad), f (coma flotante RISC-V f0-f31).
    for p in ['v', 'q', 'f'] {
        if let Some(rest) = n.strip_prefix(p) {
            if rest.parse::<u32>().map(|num| num <= 31).unwrap_or(false) {
                return true;
            }
        }
    }
    matches!(
        n,
        // x86 MMX / x87
        "mm0" | "mm1" | "mm2" | "mm3" | "mm4" | "mm5" | "mm6" | "mm7"
        | "st0" | "st1" | "st2" | "st3" | "st4" | "st5" | "st6" | "st7"
        // AVX-512 máscaras
        | "k0" | "k1" | "k2" | "k3" | "k4" | "k5" | "k6" | "k7"
        // RISC-V coma flotante (nombres ABI)
        | "ft0" | "ft1" | "ft2" | "ft3" | "ft4" | "ft5"
        | "ft6" | "ft7" | "ft8" | "ft9" | "ft10" | "ft11"
        | "fs0" | "fs1" | "fs2" | "fs3" | "fs4" | "fs5"
        | "fs6" | "fs7" | "fs8" | "fs9" | "fs10" | "fs11"
        | "fa0" | "fa1" | "fa2" | "fa3" | "fa4" | "fa5" | "fa6" | "fa7"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_purpose() {
        assert_eq!(classify("rax"), RegClass::General);
        assert_eq!(classify("R15"), RegClass::General); // case-insensitive
        assert_eq!(classify("x0"), RegClass::General);
        assert_eq!(classify("a0"), RegClass::General);
    }

    #[test]
    fn control_and_segment() {
        assert_eq!(classify("rip"), RegClass::Control);
        assert_eq!(classify("eflags"), RegClass::Control);
        assert_eq!(classify("pc"), RegClass::Control);
        assert_eq!(classify("cs"), RegClass::Segment);
        assert_eq!(classify("gs"), RegClass::Segment);
    }

    #[test]
    fn simd() {
        assert_eq!(classify("xmm0"), RegClass::Simd);
        assert_eq!(classify("ymm15"), RegClass::Simd);
        assert_eq!(classify("st3"), RegClass::Simd);
        assert_eq!(classify("v31"), RegClass::Simd);
        assert_eq!(classify("f7"), RegClass::Simd);
    }

    #[test]
    fn riscv_s_regs_stay_general_not_simd() {
        // El bug clásico: s0-s11 son enteros "saved" en RISC-V, no FP escalar.
        assert_eq!(classify("s0"), RegClass::General);
        assert_eq!(classify("s11"), RegClass::General);
        // …pero fs0 (FP) sí es SIMD.
        assert_eq!(classify("fs0"), RegClass::Simd);
    }

    #[test]
    fn unknown_is_other() {
        assert_eq!(classify("fs_base"), RegClass::Other);
        assert_eq!(classify("orig_rax"), RegClass::Other);
    }
}
