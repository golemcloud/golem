WASI System Clock is a clock API intended to let users query the current
time. The clock is not necessarily monotonic as it may be reset.

It is intended to be portable at least between Unix-family platforms and
Windows.

External references may be reset, so this clock is not necessarily
monotonic, making it unsuitable for measuring elapsed time.

It is intended for reporting the current date and time for humans.