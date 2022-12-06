# Testing Demikernel

This document contains instructions on how to test Demikernel. What it is covered:

- [What are unit tests](#what-are-unit-tests)
- [How to run unit tests](#how-to-run-unit-tests)
- [What are system-level tests](#what-are-system-level-tests)
- [How to run system-level tests](#how-to-run-system-level-tests)

> The instructions in this file assume that you have at your disposal at least one machine with Demikernel's development
environment set up. For more information on how to set up Demikernel's development environment, check out instructions
in the `README.md` file.

## What Are Unit Tests

Unit tests exercise internal components of Demikernel, individually.

Demikernel has several unit tests, and all of them spread across the source tree in `demikernel/src/`. In case you want
to check for their implementation, then search for `mod tests` and `cfg(test)` in the code base.

## How to Run Unit Tests

You can run unit tests using Demikernel's build system:

```bash
# Run all unit tests of Demikernel with Catnap LibOS.
# If you want to run unit tests on a different LibOS, then set the LIBOS flag accordingly.
make LIBOS=catnap test-unit
```

## What Are System-Level Tests

System-level tests exercise the user-level API that is exposed by Demikernel.

These tests are located in `demikernel/examples/` and detailed in the following tables.

**The following system-level tests are supported by `catcollar`, `catnap`, `catnapw`, `catnip`, `catpowder`.**

| Test Name        | Description                                                               |
|------------------|---------------------------------------------------------------------------|
| `tcp-push-pop`   | Tests for fixed size, unidirectional data transfers using TCP.            |
| `tcp-ping-pong`  | Tests for fixed size, bi-directional, symmetric data transfers using TCP. |
| `udp-push-pop`   | Tests for fixed size, unidirectional data transfers using UDP.            |
| `udp-ping-pong`  | Tests for fixed size, bi-directional, symmetric data transfers using UDP. |

**The following system-level tests are supported by `catmem`.**

| Test Name        | Description                                                      |
|------------------|------------------------------------------------------------------|
| `pipe-push-pop`  | Tests for fixed size, unidirectional data transfers.             |
| `pipe-ping-pong` | Tests for fixed size, bi-directional, symmetric data transfers.  |

## How to Run System-Level Tests

System-level tests may be run in multiple ways:

- [Using the Demikernel's CI tool (ie. `demikernel-ci`)](#running-system-level-tests-with-demikernels-ci-tool-ie-demikernel-ci). This is the preferred method.
- [Using Demikernel's build system (ie. `make`)](#running-system-level-tests-with-demikernels-build-system-ie-make). This method is discouraged, because you will have to deal by yourself with specific arguments for each test.
- [Directly from the shell (ie. `bash`)](#running-system-level-tests-from-the-shell-ie-bash). This method is strongly discouraged, because you will have to not only deal with
specific arguments for each test, but also with intricacies of your shell.

> Regardless the method you choose to follow, you should have at your disposal a pair of machines with Demikernel's
development environment set up. For more information on how to set up Demikernel's development environment, check out
instructions in the `README.md` file.

### Running System-Level Tests with Demikernel's CI Tool (ie. `demikernel-ci`)

In order to use this method, you should first do the following setup once:

- Setup RSA-based authentication for both server and host machines.
- Setup SSH aliases for connecting to both server and host machines.

```bash
# Set parameters for remote machines.
export SERVER_HOSTNAME=demikernel-vm0
export SERVER_IPV4_ADDR=10.9.1.10
export CLIENT_HOSTNAME=demikernel-vm1
export CLIENT_IPV4_ADDR=10.9.1.11
export DEMIKERNEL_PATH=/path/to/demikernel
export DEMIKERNEL_BRANCH=dev

# Run system-level tests with Catnap LibOS, unless otherwise stated.
# If you want to run unit tests on a different LibOS, then set the LIBOS flag accordingly.
export LIBOS=catnap

# Run all system tests for the target LIBOS.
python3 tools/demikernel-ci.py \
    --server $SERVER_HOSTNAME \
    --client $CLIENT_HOSTNAME \
    --repository $DEMIKERNEL_PATH \
    --branch $DEMIKERNEL_BRANCH \
    --libos $LIBOS \
    --test \
    --server-addr $SERVER_IPV4_ADDR \
    --client-addr $CLIENT_IPV4_ADDR
```

### Running System-Level Tests with Demikernel's Build System (ie. `make`)

```bash
# Set parameters for Demikernel's TCP/UDP stack.
export SERVER_IPV4_ADDR=10.9.1.10:12345
export CLIENT_IPV4_ADDR=10.9.1.11:12345

# Run system-level tests with Catnap LibOS, unless otherwise stated.
# If you want to run unit tests on a different LibOS, then set the LIBOS flag accordingly.
export LIBOS=catnap

# Run tcp-push-pop.
make test-system-rust TEST=tcp-push-pop.elf ARGS='--server $SERVER_IPV4_ADDR' # Run this on server host.
make test-system-rust TEST=tcp-push-pop.elf ARGS='--client $SERVER_IPV4_ADDR' # Run this on client host.

# Run tcp-ping-pong.
make test-system-rust TEST=tcp-ping-pong.elf ARGS='--server $SERVER_IPV4_ADDR' # Run this on server host.
make test-system-rust TEST=tcp-ping-pong.elf ARGS='--client $SERVER_IPV4_ADDR' # Run this on client host.

# Run udp-push-pop.
make test-system-rust TEST=udp-push-pop.elf ARGS='--server $SERVER_IPV4_ADDR $CLIENT_IPV4_ADDR' # Run this on server host.
make test-system-rust TEST=udp-push-pop.elf ARGS='--client $CLIENT_IPV4_ADDR $SERVER_IPV4_ADDR' # Run this on client host.

# Run udp-ping-pong.
make test-system-rust TEST=udp-ping-pong.elf ARGS='--server $SERVER_IPV4_ADDR $CLIENT_IPV4_ADDR' # Run this on server host.
make test-system-rust TEST=udp-ping-pong.elf ARGS='--client $CLIENT_IPV4_ADDR $SERVER_IPV4_ADDR' # Run this on client host.

# Run pipe-push-pop.
# - Note 1: Both commands should be executed on the same host.
# - Note 2: The 'demikernel-pipe-name' parameter is used to name the underlying pipe.
# - Note 3: You may change the pipe name to whatever you want but both ends should agree on the same name.
make test-system-rust LIBOS=catmem TEST=pipe-push-pop ARGS='--server demikernel-pipe-name'
make test-system-rust LIBOS=catmem TEST=pipe-push-pop ARGS='--client demikernel-pipe-name'

# Run pipe-ping-pong.
# - Note 1: Both commands should be executed on the same host.
# - Note 2: The 'demikernel-pipe-name' parameter is used to name the underlying pipe.
# - Note 3: You may change the pipe name to whatever you want but both ends should agree on the same name.
make test-system-rust LIBOS=catmem TEST=pipe-ping-pong ARGS='--server demikernel-pipe-name'
make test-system-rust LIBOS=catmem TEST=pipe-ping-pong ARGS='--client demikernel-pipe-name'
```

### Running System Level Tests from the Shell (ie. `bash`)

```bash
# Set location for Demikernel's config file.
export CONFIG_PATH=/path/to/config.yaml

# Set parameters for Demikernel's TCP/UDP stack.
export MSS=1500
export MTU=1500
export SERVER_IPV4_ADDR=10.9.1.10:12345
export CLIENT_IPV4_ADDR=10.9.1.11:12345

# Run system-level tests with Catnap LibOS, unless otherwise stated.
# If you want to run unit tests on a different LibOS, then set the LIBOS flag accordingly.
export LIBOS=catnap

# Run tcp-push-pop.
bin/examples/rust/tcp-push-pop.elf --server $SERVER_IPV4_ADDR # Run this on server host.
bin/examples/rust/tcp-push-pop.elf --client $SERVER_IPV4_ADDR # Run this on client host.

# Run tcp-ping-pong.
bin/examples/rust/tcp-ping-pong.elf --server $SERVER_IPV4_ADDR # Run this on server host.
bin/examples/rust/tcp-ping-pong.elf --client $SERVER_IPV4_ADDR # Run this on client host.

# Run udp-push-pop.
bin/examples/rust/udp-push-pop.elf --server $SERVER_IPV4_ADDR $CLIENT_IPV4_ADDR # Run this on server host.
bin/examples/rust/udp-push-pop.elf --client $CLIENT_IPV4_ADDR $SERVER_IPV4_ADDR # Run this on client host.

# Run udp-ping-pong.
bin/examples/rust/udp-ping-pong.elf --server $SERVER_IPV4_ADDR $CLIENT_IPV4_ADDR # Run this on server host.
bin/examples/rust/udp-ping-pong.elf --client $CLIENT_IPV4_ADDR $SERVER_IPV4_ADDR # Run this on client host.

# Run pipe-push-pop.
# - Note 1: Both commands should be executed on the same host.
# - Note 2: The 'demikernel-pipe-name' parameter is used to name the underlying pipe.
# - Note 3: You may change the pipe name to whatever you want but both ends should agree on the same name.
LIBOS=catmem bin/examples/rust/pipe-push-pop.elf --server demikernel-pipe-name
LIBOS=catmem bin/examples/rust/pipe-push-pop.elf --client demikernel-pipe-name

# Run pipe-ping-pong.
# - Note 1: Both commands should be executed on the same host.
# - Note 2: The 'demikernel-pipe-name' parameter is used to name the underlying pipe.
# - Note 3: You may change the pipe name to whatever you want but both ends should agree on the same name.
LIBOS=catmem bin/examples/rust/pipe-ping-pong.elf --server demikernel-pipe-name
LIBOS=catmem bin/examples/rust/pipe-ping-pong.elf --client demikernel-pipe-name

```
