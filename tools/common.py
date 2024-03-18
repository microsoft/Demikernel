# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

import subprocess
import time
from os import mkdir
from shutil import move, rmtree
from os.path import isdir
from typing import List
from azure.data.tables import TableServiceClient

# ======================================================================================================================
# Global Variables
# ======================================================================================================================

COMMIT_HASH: str = ""
CONNECTION_STRING: str = ""
TABLE_NAME = "test"
LIBOS = ""

# ======================================================================================================================
# Utilities
# ======================================================================================================================


def get_commit_hash() -> str:
    cmd = "git rev-parse HEAD"
    git_cmd = "bash -l -c \'{}\'".format(cmd)
    git_process = subprocess.Popen(
        git_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    git_stdout, _ = git_process.communicate()
    git_stdout = git_stdout.replace("\n", "")

    global COMMIT_HASH
    COMMIT_HASH = git_stdout
    assert len(COMMIT_HASH) == 40


def timing(f):
    def wrap(*args, **kwargs):
        time1 = time.time()
        ret = f(*args, **kwargs)
        time2 = time.time()
        duration: float = (time2-time1)*1000.0
        return (ret, duration)
    return wrap


def extract_performance(job_name, file):

    # Connect to Azure Tables.
    if not CONNECTION_STRING == "":
        table_service = TableServiceClient.from_connection_string(
            CONNECTION_STRING)
        table_client = table_service.get_table_client(TABLE_NAME)

        # Filter profiler lines.
        lines = [line for line in file if line.startswith("+")]

        # Parse statistics and upload them to azure tables.
        for line in lines:
            line = line.replace("::", ";")
            columns = line.split(";")
            # Workaround for LibOses which are miss behaving.
            if len(columns) == 6:
                syscall = columns[2]
                total_time = columns[3]
                average_cycles = columns[4]
                average_time = columns[5]

                partition_key: str = "-".join([COMMIT_HASH, LIBOS, job_name])
                row_key: str = syscall

                entry: dict[str, str, str, str, str,
                            str, float, float, float] = {}
                entry["PartitionKey"] = partition_key
                entry["RowKey"] = row_key
                entry["CommitHash"] = COMMIT_HASH
                entry["LibOS"] = LIBOS
                entry["JobName"] = job_name
                entry["Syscall"] = syscall
                entry["TotalTime"] = float(total_time)
                entry["AverageCyclesPerSyscall"] = float(average_cycles)
                entry["AverageTimePerSyscall"] = float(average_time)

                table_client.delete_entity(partition_key, row_key)
                table_client.create_entity(entry)


def wait_jobs(log_directory: str, jobs: dict):
    @timing
    def wait_jobs2(log_directory: str, jobs: dict) -> List:
        status: list[int] = []

        for job_name, j in jobs.items():
            stdout, stderr = j.communicate()
            status.append((j.pid, j.returncode))
            with open(log_directory + "/" + job_name + ".stdout.txt", "w") as file:
                file.write("{}".format(stdout))
            with open(log_directory + "/" + job_name + ".stdout.txt", "r") as file:
                extract_performance(job_name, file)

            with open(log_directory + "/" + job_name + ".stderr.txt", "w") as file:
                file.write("{}".format(stderr))

        # Cleanup list of jobs.
        jobs.clear()

        return status
    return wait_jobs2(log_directory, jobs)


def wait_and_report(name: str, log_directory: str, jobs: dict, all_pass=True):
    ret = wait_jobs(log_directory, jobs)
    passed: bool = False
    status: List = ret[0]
    duration: float = ret[1]
    if len(status) > 1:
        if all_pass:
            passed: bool = True if status[0][1] == 0 and status[1][1] == 0 else False
        else:
            passed: bool = True if status[0][1] == 0 or status[1][1] == 0 else False
    else:
        passed: bool = True if status[0][1] == 0 else False
    print("[{}] in {:9.2f} ms {}".format(
        "PASSED" if passed else "FAILED", duration, name))

    return passed

# ======================================================================================================================
# Remote Commands
# ======================================================================================================================


# Executes a checkout command in a remote host.
def remote_checkout(host: str, repository: str, branch: str):
    cmd = "cd {} && git pull origin && git checkout {}".format(
        repository, branch)
    ssh_cmd = "ssh {} \"bash -l -c \'{}\'\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Builds environment command for a remote windows host.
def build_windows_env_cmd():
    rust_path = "\$RustPath = Join-Path \$Env:HOME \\.cargo\\bin"
    git_path = "\$GitPath = Join-Path \$Env:ProgramFiles \\Git\\cmd"
    env_path_git = "\$Env:Path += \$GitPath + \';\'"
    env_path_rust = "\$Env:Path += \$RustPath + \';\'"
    vs_install_path = "\$VsInstallPath = &(Join-Path \${Env:ProgramFiles(x86)} '\\Microsoft Visual Studio\\Installer\\vswhere.exe') -latest -property installationPath"
    import_module = "Import-Module (Join-Path \$VsInstallPath 'Common7\\Tools\\Microsoft.VisualStudio.DevShell.dll')"
    enter_vsdevshell = "Enter-VsDevShell -VsInstallPath \$VsInstallPath -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'"

    env_cmd = " ; ".join([rust_path, git_path, env_path_git, env_path_rust, vs_install_path,
                          import_module, enter_vsdevshell])
    return env_cmd


# Executes a checkout command in a remote windows host.
def remote_checkout_windows(host: str, repository: str, branch: str):
    env_cmd = build_windows_env_cmd()
    cmd = "cd {} ; {} ; git pull origin ; git checkout {}".format(
        repository, env_cmd, branch)
    ssh_cmd = "ssh {} \"{}\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Executes a compile command in a remote host.
def remote_compile(host: str, repository: str, target: str, is_debug: bool):
    debug_flag: str = "DEBUG=yes" if is_debug else "DEBUG=no"
    profiler_flag: str = "PROFILER=yes" if not is_debug else "PROFILER=no"
    cmd = "cd {} && make {} {} {}".format(
        repository, profiler_flag, debug_flag, target)
    ssh_cmd = "ssh {} \"bash -l -c \'{}\'\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Executes a compile command in a remote windows host.
def remote_compile_windows(host: str, repository: str, target: str, is_debug: bool):
    env_cmd = build_windows_env_cmd()
    debug_flag: str = "DEBUG=yes" if is_debug else "DEBUG=no"
    profiler_flag: str = "PROFILER=yes" if not is_debug else "PROFILER=no"
    cmd = "cd {} ; {} ; nmake {} {} {}".format(
        repository, env_cmd, profiler_flag, debug_flag, target)
    ssh_cmd = "ssh {} \"{}\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Executes a test in a remote host.
def remote_run(host: str, repository: str, is_debug: bool, target: str, is_sudo: bool, config_path: str):
    debug_flag: str = "DEBUG=yes" if is_debug else "DEBUG=no"
    sudo_cmd: str = "sudo -E" if is_sudo else ""
    cmd = "cd {} && {} make -j 1 CONFIG_PATH={} {} {} 2> out.stderr && cat out.stderr >&2 || ( cat out.stderr >&2 ; exit 1 )".format(
        repository, sudo_cmd, config_path, debug_flag, target)
    ssh_cmd = "ssh {} \"bash -l -c \'{}\'\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Executes a test in a remote windows host.
def remote_run_windows(host: str, repository: str, is_debug: bool, target: str, is_sudo: bool, config_path: str):
    env_cmd = build_windows_env_cmd()
    debug_flag: str = "DEBUG=yes" if is_debug else "DEBUG=no"
    cmd = "cd {} ; {} ; nmake CONFIG_PATH={} {} {}".format(
        repository, env_cmd, config_path, debug_flag, target)
    ssh_cmd = "ssh {} \"{}\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Executes a cleanup command in a remote host.
def remote_cleanup(host: str, workspace: str, is_sudo: bool, default_branch: str = "dev"):
    sudo_cmd: str = "sudo -E" if is_sudo else ""
    cmd = "cd {} && {} make clean && git checkout {} && git clean -fdx ; sudo -E rm -rf /dev/shm/demikernel* ; sudo pkill -f demikernel*".format(
        workspace, sudo_cmd, default_branch)
    ssh_cmd = "ssh {} \"bash -l -c \'{}\'\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# Executes a cleanup command in a remote windows host.
def remote_cleanup_windows(host: str, workspace: str, is_sudo: bool, default_branch: str = "dev"):
    env_cmd = build_windows_env_cmd()
    cmd = "cd {} ; {} ; nmake clean ; git checkout ; git clean -fdx".format(
        workspace, env_cmd, default_branch)
    ssh_cmd = "ssh {} \"{}\"".format(host, cmd)
    return subprocess.Popen(ssh_cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


# ======================================================================================================================
# Generic Jobs
# ======================================================================================================================


def job_checkout(repository: str, branch: str, server: str, client: str, enable_nfs: bool,
                 log_directory: str) -> bool:
    # Jobs is a map of job names (server name, repository and compile mode)
    jobs: dict[str, subprocess.Popen[str]] = {}
    test_name = "checkout"
    jobs[test_name + "-server-" +
         server] = remote_checkout(server, repository, branch)
    if not enable_nfs:
        jobs[test_name + "-client-" +
             client] = remote_checkout(client, repository, branch)
    return wait_and_report(test_name, log_directory, jobs)


def job_checkout_windows(repository: str, branch: str, server: str, client: str, enable_nfs: bool,
                         log_directory: str) -> bool:
    # Jobs is a map of job names (server name, repository and compile mode)
    jobs: dict[str, subprocess.Popen[str]] = {}
    test_name = "checkout"
    jobs[test_name + "-server-" +
         server] = remote_checkout_windows(server, repository, branch)
    if not enable_nfs:
        jobs[test_name + "-client-" +
             client] = remote_checkout_windows(client, repository, branch)
    return wait_and_report(test_name, log_directory, jobs)


def job_compile(
        repository: str, libos: str, is_debug: bool, server: str, client: str, enable_nfs: bool,
        log_directory: str) -> bool:
    jobs: dict[str, subprocess.Popen[str]] = {}
    test_name = "compile-{}".format("debug" if is_debug else "release")
    jobs[test_name + "-server-" + server] = remote_compile(
        server, repository, "all LIBOS={}".format(libos), is_debug)
    if not enable_nfs:
        jobs[test_name + "-client-" + client] = remote_compile(client,
                                                               repository, "all LIBOS={}".format(libos), is_debug)
    return wait_and_report(test_name, log_directory, jobs)


def job_compile_windows(
        repository: str, libos: str, is_debug: bool, server: str, client: str, enable_nfs: bool,
        log_directory: str) -> bool:
    jobs: dict[str, subprocess.Popen[str]] = {}
    test_name = "compile-{}".format("debug" if is_debug else "release")
    jobs[test_name + "-server-" + server] = remote_compile_windows(
        server, repository, "all LIBOS={}".format(libos), is_debug)
    if not enable_nfs:
        jobs[test_name + "-client-" + client] = remote_compile_windows(client,
                                                                       repository, "all LIBOS={}".format(libos), is_debug)
    return wait_and_report(test_name, log_directory, jobs)


def job_test_system_rust(
        test_alias: str, test_name: str, repo: str, libos: str, is_debug: bool, server: str, client: str,
        server_args: str, client_args: str, is_sudo: bool, all_pass: bool, delay: float, config_path: str,
        log_directory: str) -> bool:
    server_cmd: str = "test-system-rust LIBOS={} TEST={} ARGS=\\\"{}\\\"".format(
        libos, test_name, server_args)
    client_cmd: str = "test-system-rust LIBOS={} TEST={} ARGS=\\\"{}\\\"".format(
        libos, test_name, client_args)
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_alias + "-server-" +
         server] = remote_run(server, repo, is_debug, server_cmd, is_sudo, config_path)
    time.sleep(delay)
    jobs[test_alias + "-client-" +
         client] = remote_run(client, repo, is_debug, client_cmd, is_sudo, config_path)
    return wait_and_report(test_alias, log_directory, jobs, all_pass)


def job_test_system_rust_windows(
        test_alias: str, test_name: str, repo: str, libos: str, is_debug: bool, server: str, client: str,
        server_args: str, client_args: str, is_sudo: bool, all_pass: bool, delay: float, config_path: str,
        log_directory: str) -> bool:
    server_cmd: str = "test-system-rust LIBOS={} TEST={} ARGS='{}'".format(
        libos, test_name, server_args)
    client_cmd: str = "test-system-rust LIBOS={} TEST={} ARGS='{}'".format(
        libos, test_name, client_args)
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_alias + "-server-" +
         server] = remote_run_windows(server, repo, is_debug, server_cmd, is_sudo, config_path)
    time.sleep(delay)
    jobs[test_alias + "-client-" +
         client] = remote_run_windows(client, repo, is_debug, client_cmd, is_sudo, config_path)
    return wait_and_report(test_alias, log_directory, jobs, all_pass)


def job_test_unit_rust(repo: str, libos: str, is_debug: bool, server: str, client: str,
                       is_sudo: bool, config_path: str, log_directory: str) -> bool:
    server_cmd: str = "test-unit-rust LIBOS={}".format(libos)
    client_cmd: str = "test-unit-rust LIBOS={}".format(libos)
    test_name = "unit-test"
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_run(server, repo, is_debug, server_cmd, is_sudo, config_path)
    # Unit tests require a single endpoint, so do not run them on client.
    return wait_and_report(test_name, log_directory, jobs, True)


def job_test_unit_rust_windows(repo: str, libos: str, is_debug: bool, server: str, client: str,
                               is_sudo: bool, config_path: str, log_directory: str) -> bool:
    server_cmd: str = "test-unit-rust LIBOS={}".format(libos)
    client_cmd: str = "test-unit-rust LIBOS={}".format(libos)
    test_name = "unit-test"
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_run_windows(server, repo, is_debug, server_cmd, is_sudo, config_path)
    # Unit tests require a single endpoint, so do not run them on client.
    return wait_and_report(test_name, log_directory, jobs, True)


def job_test_integration_tcp_rust(
        repo: str, libos: str, is_debug: bool, server: str, client: str, server_addr: str, client_addr: str,
        is_sudo: bool, config_path: str, log_directory: str) -> bool:
    server_args: str = "--local-address {}:12345 --remote-address {}:23456".format(
        server_addr, client_addr)
    client_args: str = "--local-address {}:23456 --remote-address {}:12345".format(
        client_addr, server_addr)
    server_cmd: str = "test-integration-rust TEST_INTEGRATION=tcp-test LIBOS={} ARGS=\\\"{}\\\"".format(
        libos, server_args)
    client_cmd: str = "test-integration-rust TEST_INTEGRATION=tcp-test LIBOS={} ARGS=\\\"{}\\\"".format(
        libos, client_args)
    test_name = "integration-test"
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_run(server, repo, is_debug, server_cmd, is_sudo, config_path)
    if libos != "catloop":
        jobs[test_name + "-client-" + client] = remote_run(
            client, repo, is_debug, client_cmd, is_sudo, config_path)
    return wait_and_report(test_name, log_directory, jobs, True)


def job_test_integration_tcp_rust_windows(
        repo: str, libos: str, is_debug: bool, server: str, client: str, server_addr: str, client_addr: str,
        is_sudo: bool, config_path: str, log_directory: str) -> bool:
    server_args: str = "--local-address {}:12345 --remote-address {}:23456".format(
        server_addr, client_addr)
    client_args: str = "--local-address {}:23456 --remote-address {}:12345".format(
        client_addr, server_addr)
    server_cmd: str = "test-integration-rust TEST_INTEGRATION=tcp-test LIBOS={} ARGS='{}'".format(
        libos, server_args)
    client_cmd: str = "test-integration-rust TEST_INTEGRATION=tcp-test LIBOS={} ARGS='{}'".format(
        libos, client_args)
    test_name = "integration-test"
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_run_windows(server, repo, is_debug, server_cmd, is_sudo, config_path)
    jobs[test_name + "-client-" + client] = remote_run_windows(
        client, repo, is_debug, client_cmd, is_sudo, config_path)
    return wait_and_report(test_name, log_directory, jobs, True)


def job_test_integration_pipe_rust(
        repo: str, libos: str, is_debug: bool, run_mode: str, server: str, client: str, server_addr: str,
        delay: float, is_sudo: bool, config_path: str, log_directory: str) -> bool:
    server_args: str = "--peer server --pipe-name {}:12345 --run-mode {}".format(
        server_addr, run_mode)
    client_args: str = "--peer client --pipe-name {}:12345 --run-mode {}".format(
        server_addr, run_mode)
    server_cmd: str = "test-integration-rust TEST_INTEGRATION=pipe-test LIBOS={} ARGS=\\\"{}\\\"".format(
        libos, server_args)
    client_cmd: str = "test-integration-rust TEST_INTEGRATION=pipe-test LIBOS={} ARGS=\\\"{}\\\"".format(
        libos, client_args)
    test_name = "integration-test" + "-" + run_mode
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_run(server, repo, is_debug, server_cmd, is_sudo, config_path)
    if run_mode != "standalone":
        time.sleep(delay)
        jobs[test_name + "-client-" + client] = remote_run(
            client, repo, is_debug, client_cmd, is_sudo, config_path)
    return wait_and_report(test_name, log_directory, jobs, True)


def job_cleanup(repository: str, server: str, client: str, is_sudo: bool, enable_nfs: bool, log_directory: str) -> bool:
    test_name = "cleanup"
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_cleanup(server, repository, is_sudo)
    if not enable_nfs:
        jobs[test_name + "-client-" + client +
             "-"] = remote_cleanup(client, repository, is_sudo)
    return wait_and_report(test_name, log_directory, jobs)


def job_cleanup_windows(repository: str, server: str, client: str, is_sudo: bool, enable_nfs: bool, log_directory: str) -> bool:
    test_name = "cleanup"
    jobs: dict[str, subprocess.Popen[str]] = {}
    jobs[test_name + "-server-" +
         server] = remote_cleanup_windows(server, repository, is_sudo)
    if not enable_nfs:
        jobs[test_name + "-client-" + client +
             "-"] = remote_cleanup_windows(client, repository, is_sudo)
    return wait_and_report(test_name, log_directory, jobs)