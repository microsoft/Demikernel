# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

from io import StringIO
import argparse
import ci.git as git
import glob
import os
import pandas as pd
import subprocess


def main():
    args: argparse.Namespace = __read_args()
    __build_report(args)


def __read_args() -> argparse.Namespace:
    description: str = 'CI Utility for reporting performance statistics of Demikernel.'
    parser = argparse.ArgumentParser(prog='plot.py', description=description)
    parser.add_argument('--branch', required=True, help='Current branch name.')
    parser.add_argument('--libos', required=False, help='LibOS Name.')
    parser.add_argument('--log-dir', required=False, help='The directory where the logs are stored.')
    return parser.parse_args()


def __build_report(args):
    commit_id = git.get_head_commit(args.branch)

    # This information is printed to the console and will be used by the
    # workflow to post a comment on the PR.
    print('libos = ' + args.libos)
    print('commit id = ' + commit_id)

    perf_df = __get_perf_data(args.log_dir)
    __compute_percentages(perf_df)
    __print_perf_data(perf_df)
    __create_flame_graph(args.libos, commit_id, perf_df)
    # This file will be used by the workflow to archive the perf data.
    perf_df.to_csv('perf_data.csv', index=False, header=True)


def __get_perf_data(log_dir):
    files = glob.glob(os.path.join(log_dir, '**', 'system-test*stdout*'), recursive=True)
    collapsed_stacks = []

    for file in files:
        with open(file, 'r') as f:
            __populate_collapsed_stacks(collapsed_stacks, f)

    perf_df = pd.read_csv(
        StringIO('\n'.join(collapsed_stacks)),
        names=['collapsed_stack',
               'num_calls',
               'cycles_per_call',
               'nanoseconds_per_call',
               'total_duration',
               'total_duration_exclusive'])

    # There will be multiple entries for each function in the perf data coming from different files. So, we need to
    # collapse them into a single entry by computing the mean of the values.
    perf_df = perf_df.groupby(by='collapsed_stack').mean().round().fillna(0).astype(int).reset_index()
    return perf_df


def __compute_percentages(perf_df):
    perf_df['percent_total_duration'] = round(
        100 * perf_df['total_duration'] / perf_df['total_duration'].sum()
    ).astype(int)
    perf_df['percent_total_duration_exclusive'] = round(
        100 * perf_df['total_duration_exclusive'] / perf_df['total_duration_exclusive'].sum()
    ).astype(int)


def __populate_collapsed_stacks(collapsed_stacks, file):
    file_df = __get_file_df(file)
    thread_ids = file_df['thread_id'].unique()

    for thread_id in thread_ids:
        # The current stack is used to keep track of the current function call
        # stack. This sort of mimics the call stack of the program being profiled.
        current_stack = []

        for _, row in file_df[file_df['thread_id'] == thread_id].iterrows():
            depth = row['call_depth']

            # The current stack is reset when the depth of the function call is
            # 1. This denotes the start of a new function call stack.
            current_stack = [] if depth == 1 else current_stack[:depth-1]
            current_stack.append(row['function_name'])

            # The collapsed stack is a string that contains the function names
            # separated by a semicolon.
            collapsed_stack = ';'.join(current_stack)
            collapsed_stacks.append(f"{collapsed_stack},{row['num_calls']},{row['cycles_per_call']},{row['nanoseconds_per_call']},{row['total_duration']},{row['total_duration_exclusive']}")


def __get_file_df(file):
    lines = __extract_perf_lines(file)
    file_df = pd.read_csv(
        StringIO('\n'.join(lines)),
        delimiter=',',
        names=['call_depth',
               'thread_id',
               'function_name',
               'num_calls',
               'cycles_per_call',
               'nanoseconds_per_call',
               'total_duration',
               'total_duration_exclusive',
               ])
    # Number of '+' characters in the call_depth column denotes the depth of the function call.
    file_df['call_depth'] = file_df['call_depth'].apply(lambda x: x.count('+')).astype(int)
    return file_df


def __extract_perf_lines(file):
    lines = []
    for line in file:
        # Lines that don't start with a '+' character are not perf data.
        if not line.startswith('+'):
            continue
        lines.append(line)
    return lines


def __print_perf_data(perf_df):
    sort_by_columns = [
        'total_duration_exclusive',
        'num_calls',
        'cycles_per_call',
        'nanoseconds_per_call',
        'total_duration',
    ]
    columns_to_display = [
        'collapsed_stack',
        'num_calls',
        'cycles_per_call',
        'nanoseconds_per_call',
        'total_duration',
        'percent_total_duration',
        'total_duration_exclusive',
        'percent_total_duration_exclusive',
    ]

    out_df = perf_df.sort_values(by=sort_by_columns, ascending=False)[columns_to_display]
    # This print will be used by the workflow to post a comment on the PR.
    print(out_df.to_markdown(floatfmt='.2f', index=False))


def __create_flame_graph(libos, commit_id, perf_df) -> None:
    # Save folded stacks to file for consumption by flamegraph.pl
    perf_df[['collapsed_stack', 'total_duration_exclusive']].to_csv(
        'flamegraph_input.txt', index=False, sep=' ', header=False)
    # Render flame graph
    subprocess.run(['/tmp/FlameGraph/flamegraph.pl', 'flamegraph_input.txt',
                    '--countname', 'total_duration_exclusive',
                    '--title', 'libos = ' + libos,
                    '--subtitle', 'commit id = ' + commit_id],
                   check=True,
                   stdout=open('flamegraph.svg', 'w'))


if __name__ == '__main__':
    main()
