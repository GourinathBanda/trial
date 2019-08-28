use core::ptr;

use crate::config::{MAX_STACK_SIZE, MAX_TASKS, SYSTICK_INTERRUPT_INTERVAL};
use crate::errors::KernelError;
use cortex_m::interrupt::free as execute_critical;
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_semihosting::hprintln;

pub type TaskId = u32;

#[repr(C)]
struct TaskManager {
    // start fields used in assembly, do not change their order
    ptr_RT: usize, // pointer to running task
    ptr_HT: usize, // pointer to current high priority task (or the next task to be scheduled)
    // end fields used in assembly
    RT: usize,
    is_running: bool,
    threads: [Option<TaskControlBlock>; MAX_TASKS],
    BTV: u32,
    ATV: u32,
    is_preemptive: bool,
}

/// A single thread's state
#[repr(C)]
#[derive(Clone, Copy)]
struct TaskControlBlock {
    // fields used in assembly, do not reorder them
    sp: usize, // current stack pointer of this thread
}

// GLOBALS:
#[no_mangle]
static mut __CORTEXM_THREADS_GLOBAL_PTR: u32 = 0;
#[no_mangle]
static mut __CORTEXM_THREADS_GLOBAL: TaskManager = TaskManager {
    ptr_RT: 0,
    ptr_HT: 0,
    RT: 50,
    is_running: false,
    threads: [None; MAX_TASKS],
    ATV: 1,
    BTV: 0,
    is_preemptive: false,
};
#[no_mangle]
static mut TASK_STACKS: [[u32; MAX_STACK_SIZE]; MAX_TASKS] = [[0; MAX_STACK_SIZE]; MAX_TASKS];
// end GLOBALS

/// Initialize the switcher system
pub fn init(is_preemptive: bool) {
    execute_critical(|_| {
        unsafe {
            let ptr: usize = core::intrinsics::transmute(&__CORTEXM_THREADS_GLOBAL);
            __CORTEXM_THREADS_GLOBAL_PTR = ptr as u32;
            __CORTEXM_THREADS_GLOBAL.is_preemptive = is_preemptive;
        }
        /*
            This is the default task, that just puts the board for a power-save mode
            until any event (interrupt/exception) occurs.
        */
        create_task(0, || loop {
            cortex_m::asm::wfe();
        })
        .unwrap();
    });
}

// The below section just sets up the timer and starts it.
pub fn start_kernel() -> Result<(), KernelError> {
    execute_critical(|_| {
        let cp = cortex_m::Peripherals::take().unwrap();
        let mut syst = cp.SYST;
        syst.set_clock_source(SystClkSource::Core);
        syst.set_reload(SYSTICK_INTERRUPT_INTERVAL);
        syst.enable_counter();
        syst.enable_interrupt();
        unsafe {
            __CORTEXM_THREADS_GLOBAL.is_running = true;
        }
        preempt()?;
        return Ok(());
    })
}

pub fn release(tasks_mask: &u32) {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        handler.ATV |= *tasks_mask;
        preempt();
    });
}

pub fn create_task(priority: usize, handler_fn: fn() -> !) -> Result<(), KernelError> {
    execute_critical(|_| {
        let mut stack = unsafe { &mut TASK_STACKS[priority] };
        match create_tcb(stack, handler_fn, true) {
            Ok(tcb) => {
                insert_tcb(priority, tcb)?;
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    })
}

pub fn preempt() -> Result<(), KernelError> {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        if handler.is_running {
            let HT = get_HT();
            // schedule a thread to be run
            if handler.RT != HT {
                handler.RT = HT;
                let task = &handler.threads[handler.RT];
                if let Some(task) = task {
                    unsafe {
                        handler.ptr_HT = core::intrinsics::transmute(task);
                        // The following Causes PendSV interrupt, the interrupt handler is written in assembly
                        let pend = ptr::read_volatile(0xE000ED04 as *const u32);
                        ptr::write_volatile(0xE000ED04 as *mut u32, pend | 1 << 28);
                    }
                } else {
                    return Err(KernelError::DoesNotExist);
                }
            }
        }
        return Ok(());
    })
}

fn get_HT() -> usize {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        for i in (1..MAX_TASKS as u32).rev() {
            let i_mask = (1 << i);
            if (handler.ATV & i_mask == i_mask) && (handler.BTV & i_mask != i_mask) {
                return i as usize;
            }
        }
        return 0;
    })
}

fn create_tcb(
    stack: &mut [u32],
    handler: fn() -> !,
    priviliged: bool,
) -> Result<TaskControlBlock, KernelError> {
    execute_critical(|_| {
        if stack.len() < 32 {
            return Err(KernelError::StackTooSmall);
        }

        let idx = stack.len() - 1;
        stack[idx] = 1 << 24; // xPSR
        let pc: usize = handler as usize;
        stack[idx - 1] = pc as u32; // PC

        let sp: usize = unsafe { core::intrinsics::transmute(&stack[stack.len() - 16]) };
        let tcb = TaskControlBlock { sp: sp as usize };
        Ok(tcb)
    })
}

fn insert_tcb(idx: usize, tcb: TaskControlBlock) -> Result<(), KernelError> {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        if idx >= MAX_TASKS {
            return Err(KernelError::DoesNotExist);
        }
        handler.threads[idx] = Some(tcb);
        return Ok(());
    })
}

pub fn is_preemptive() -> bool {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        handler.is_preemptive
    })
}

pub fn get_RT() -> usize {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        return handler.RT;
    })
}

pub fn block_tasks(tasks_mask: u32) {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        handler.BTV |= tasks_mask;
    })
}

pub fn unblock_tasks(tasks_mask: u32) {
    execute_critical(|_| {
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        handler.BTV &= !tasks_mask;
    })
}

pub fn task_exit() {
    execute_critical(|_| {
        let rt = get_RT();
        let handler = unsafe { &mut __CORTEXM_THREADS_GLOBAL };
        handler.ATV &= !(1 << rt as u32);
        preempt().unwrap();
    })
}

pub fn release_tasks(tasks: &[TaskId]) {
    execute_critical(|_| {
        let mut mask = 0;
        for tid in tasks {
            mask |= 1 << *tid;
        }
        release(&mask);
    })
}

#[macro_export]
macro_rules! spawn {
    ($task_name: ident, $priority: expr, $handler_fn: block) => {
        create_task($priority,|| loop {
            $handler_fn
            task_exit();
        }).unwrap();
        static $task_name: TaskId = $priority;
    }
}