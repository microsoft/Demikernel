# `demi_init(2)`

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

The `argv` parameter is an array of argument strings passed to Demikernel. The `argc` parameter is a positive integer
that specifies the length of that array.

The array of argument strings provides Demikernel with various information that are critical for its initialization. For
instance, these arguments shall state which LibOSes should be initialized, and whether or not runtime features shall be
turned on.

All arguments supported by Demikernel are listed in the table bellow. Arguments that are not listed therein are ignored.

|ID       | Argument      | Description             |
|---------|---------------|-------------------------|
| `ARG-1` | `--catcollar` | Enables Catcollar LibOS |
| `ARG-2` | `--catnap`    | Enables Catnap LibOS    |
| `ARG-3` | `--catnip`    | Enables Catnip LibOS    |
| `ARG-4` | `--catpowder` | Enables Catpowder LibOS |

 Any constraints/restrictions of arguments are detailed next:

- `ARG-1` takes effect only on Linux hosts. If one is attempts to initialize Catcollar on a non-Linux host and `ARG-1`
is the only parameter passed to Demikernel, `demi_init()` will fail.

- `ARG-1`, `ARG-2`, `ARG-3` and `ARG-4` are mutually exclusive. Demikernel currently does not support multiple LibOSes
to co-exist. [Issue #158](https://github.com/demikernel/demikernel/issues/158) tracks progress of this feature.

## Return Value

On success, zero is returned. On error, a positive error code is returned and any subsequent call to Demikernel may
result in unexpected behavior.

## Errors

On error, one of the following positive error codes is returned:

- `EINVAL` - The `argc` parameter is less than or equal to zero.
- `EINVAL` - The `argv` parameter is `NULL`.

## Conforming To

Error codes are conformant to [POSIX.1-2017](https://pubs.opengroup.org/onlinepubs/9699919799/nframe.html).

## Bugs

Demikernel may fail with error codes that are not listed in this manual page.

## Disclaimer

Any behavior that is not documented in this manual page is unintentional and should be reported.
