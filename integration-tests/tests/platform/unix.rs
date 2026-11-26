pub(crate) fn enable_stack_overflow_backtrace() {
    unsafe { backtrace_on_stack_overflow::enable() };
}
