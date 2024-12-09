# `demi_init()`

## Name

`demi_init` - Initializes Demikernel.

## Synopsis

```c
#include <demi/libos.h>

int demi_init(int argc, char *const argv[]);
```

## Description

`demi_init()` initializes Demikernel. It sets up devices, instantiates LibOSes, and performs general initialization
tasks.

## Return Value

On success, zero is returned. On error, a positive error code is returned and any subsequent call to Demikernel may
result in unexpected behavior.

## Errors

On error, one of the following positive error codes is returned:

- `EINVAL` - The `argc` argument is less than or equal to zero.
- `EINVAL` - The `argv` argument is `NULL`.
- `EEXIST` - The LibOS has already been initialized.

## Conforming To

Error codes are conformant to [POSIX.1-2017](https://pubs.opengroup.org/onlinepubs/9699919799/nframe.html).

## Bugs

Demikernel may fail with error codes that are not listed in this manual page.

## Disclaimer

Any behavior that is not documented in this manual page is unintentional and should be reported.
