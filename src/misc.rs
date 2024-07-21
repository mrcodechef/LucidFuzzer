//! This file contains miscellaneous helper functions

use core::arch::x86_64::{_fxrstor64, _fxsave64, _xgetbv, _xrstor64, _xsave64};

use crate::err::LucidErr;

#[macro_export]
macro_rules! prompt {
    () => ({
        print!("\x1b[1;35m[lucid]\x1b[0m\n");
    });
    ($($arg:tt)*) => ({
        print!("\x1b[1;35m[lucid]\x1b[0m ");
        println!($($arg)*);
    });
}

#[macro_export]
macro_rules! prompt_warn {
    () => ({
        print!("\x1b[1;33m[lucid]\x1b[0m\n");
    });
    ($($arg:tt)*) => ({
        print!("\x1b[1;33m[lucid]\x1b[0m ");
        println!($($arg)*);
    });
}

#[macro_export]
macro_rules! finding {
    ($id:expr) => ({
        print!("\x1b[1;37mfuzzer-{}:\x1b[0m\n", $id);
    });
    ($id:expr, $($arg:tt)*) => ({
        print!("\x1b[1;37mfuzzer-{}:\x1b[0m ", $id);
        println!($($arg)*);
    });
}

#[macro_export]
macro_rules! finding_warn {
    ($id:expr) => ({
        print!("\x1b[1;37mfuzzer-{}:\x1b[0m\n", $id);
    });
    ($id:expr, $($arg:tt)*) => ({
        print!("\x1b[1;37mfuzzer-{}:\x1b[0m ", $id);
        println!($($arg)*);
    });
}

#[macro_export]
macro_rules! fatal {
    ($err:expr) => {{
        print!("\n\x1b[1;31mfatal:\x1b[0m ");
        $err.display();
        std::process::exit(-1);
    }};
}

// Hides `unreachable!()`
#[macro_export]
macro_rules! fault {
    ($contextp:expr, $fault:expr) => {{
        fault_handler($contextp, $fault);
        unreachable!();
    }};
}

#[macro_export]
macro_rules! green {
    () => {{
        print!("\x1b[1;32m");
    }};
}

#[macro_export]
macro_rules! red {
    () => {{
        print!("\x1b[1;31m");
    }};
}

#[macro_export]
macro_rules! clear {
    () => {{
        print!("\x1b[0m");
    }};
}

#[macro_export]
macro_rules! mega_panic {
    ($msg:expr) => {{
        use core::arch::asm;

        // Length of the message
        let msg_len = $msg.len();
        let msg_ptr = $msg.as_ptr();
        let stderr: usize = 2;

        // Unsafe block for inline assembly
        unsafe {
            // Write the message to stderr
            asm!(
                "mov rax, 1",               // syscall number for sys_write
                "mov rdi, {0}",             // file descriptor (stderr)
                "mov rsi, {1}",             // pointer to the message
                "mov rdx, {2}",             // length of the message
                "syscall",                  // make the syscall
                in(reg) stderr,
                in(reg) msg_ptr,
                in(reg) msg_len,
                out("rax") _, out("rdi") _, out("rsi") _, out("rdx") _,
            );

            // Call the exit syscall with an exit code
            asm!(
                "mov rax, 60",              // syscall number for sys_exit
                "mov rdi, 1",               // exit code 1 (error)
                "syscall",                  // make the syscall
                options(noreturn),
            );
        }
    }};
}

// Wrappers for these unsafe functions to tuck unsafes away
pub fn get_xcr0() -> u64 {
    unsafe { _xgetbv(0) }
}

pub fn xsave64(save_area: *mut u8, xcr0: u64) {
    unsafe { _xsave64(save_area, xcr0) }
}

pub fn fxsave64(save_area: *mut u8) {
    unsafe { _fxsave64(save_area) }
}

pub fn xrstor64(save_area: *const u8, xcr0: u64) {
    unsafe { _xrstor64(save_area, xcr0) }
}

pub fn fxrstor64(save_area: *const u8) {
    unsafe { _fxrstor64(save_area) }
}

// Pin a process to a specific CPU core
pub fn pin_core(core: usize) {
    unsafe {
        let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut cpuset);
        libc::CPU_SET(core, &mut cpuset);

        let result = libc::sched_setaffinity(
            0, // 0 means current process
            std::mem::size_of::<libc::cpu_set_t>(),
            &cpuset,
        );

        if result != 0 {
            fatal!(LucidErr::from("Failed to pin fuzzer to core"));
        }
    }
}

// Waitpid for non-blocking
pub fn non_block_waitpid(pid: i32, status: &mut i32) -> i32 {
    unsafe { libc::waitpid(pid, status, libc::WNOHANG) }
}

// Handle waitpid result
pub fn handle_wait_result(result: i32, status: &i32) -> Result <(), ()> {
    match result {
        1.. => {
            if libc::WIFEXITED(*status) {
                let exit = libc::WEXITSTATUS(*status);
                prompt_warn!("Child fuzzer exited with status: {}", exit);
                return Err(());
            } else if libc::WIFSIGNALED(*status) {
                let signal = libc::WTERMSIG(*status);
                prompt_warn!("Child fuzzer was signaled with: {}", signal);
                return Err(());
            }

            // Unknown cause?
            prompt_warn!("Child fuzzer was stopped, we don't know why");
            return Err(());
        }
        -1 => {
            prompt_warn!("Error from calling waitpid on child fuzzer");
            return Err(());
        }
        _ => (), // No change, good!
    }

    Ok(())
}
