// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#ifndef DEMI_WAIT_H_IS_INCLUDED
#define DEMI_WAIT_H_IS_INCLUDED

#include <demi/types.h>
#include <time.h>

#ifdef __cplusplus
extern "C"
{
#endif

    /**
     * @brief Waits for an asynchronous I/O operation to complete.
     *
     * @param qr_out  Store location for the result of the completed I/O operation.
     * @param qt      I/O queue token of the target operation to wait for completion.
     * @param timeout Timeout interval in seconds and nanoseconds.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_wait(_Out_ demi_qresult_t *qr_out, _In_ demi_qtoken_t qt, _In_opt_ const struct timespec *timeout)
        ATTR_NONNULL(1);

    /**
     * @brief Waits for the first asynchronous I/O operation in a list to complete.
     *
     * @param qr_out       Store location for the result of the completed I/O operation.
     * @param ready_offset Store location for the offset in the list of I/O queue tokens of the completed I/O operation.
     * @param qts          List of I/O queue tokens to wait for completion.
     * @param num_qts      Length of the list of I/O queue tokens to wait for completion.
     * @param timeout      Timeout interval in seconds and nanoseconds.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_wait_any(_Out_ demi_qresult_t *qr_out, _Out_ int *ready_offset,
                             _In_reads_(num_qts) const demi_qtoken_t qts[], _In_ int num_qts,
                             _In_opt_ const struct timespec *timeout)
        ATTR_NONNULL(1, 2, 3);

    /**
     * @brief Waits for the next n asynchronous I/O operations to complete.
     *
     * @param qr_out       Store location for the result of the completed I/O operation.
     * @param ready_offset Store location for the offset in the list of I/O queue tokens of the completed I/O operation.
     * @param qts          List of I/O queue tokens to wait for completion.
     * @param num_qts      Length of the list of I/O queue tokens to wait for completion.
     * @param timeout      Timeout interval in seconds and nanoseconds.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_wait_any(_Out_writes_to_(num_qrs, *ready_offset) demi_qresult_t *qr_out, _In_ int num_qrs,
                             _Out_ int *ready_offset, _In_opt_ const struct timespec *timeout)
        ATTR_NONNULL(1, 3);

#ifdef __cplusplus
}
#endif

#endif /* DEMI_WAIT_H_IS_INCLUDED */
