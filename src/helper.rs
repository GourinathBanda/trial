use crate::config::MAX_TASKS;

pub fn generate_task_mask(tasks: &[u32]) -> u32 {
    let mut task_mask = 0;
    for tid in tasks {
        task_mask |= 1 << *tid;
    }
    task_mask
}

pub fn get_msb(val: &u32) -> usize {
    for i in (0..MAX_TASKS).rev() {
        let mut mask = 0;
        mask <<= i;
        if val & mask == mask {
            return i;
        }
    }
    return 0;
}