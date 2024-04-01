// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#ifndef DEMI_LIBOS_H_IS_INCLUDED
#define DEMI_LIBOS_H_IS_INCLUDED

#include <demi/types.h>
#include <stddef.h>

#ifdef __linux__
#include <sys/socket.h>
#endif

#ifdef _WIN32
#include <winsock.h>
typedef int socklen_t;
#endif

#ifdef __cplusplus
extern "C"
{
#endif

    /**
     * @brief Initializes Demikernel.
     *
     * @param argc Number of arguments.
     * @param argv Argument values.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_init(_In_ int argc, _In_reads_(argc) _Deref_pre_z_ char *const argv[])
        ATTR_NONNULL(1);


    /**
     * @brief Creates a new memory I/O queue.
     *
     * @param memqd_out Storage location for the memory I/O queue descriptor
     * @param name      Name of the target memory I/O queue.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_create_pipe(_Out_ int *memqd_out, _In_z_ const char *name)
        ATTR_NONNULL(0, 1);

    /**
     * @brief Opens an existing memory I/O queue.
     *
     * @param memqd_out Storage location for the memory I/O queue descriptor
     * @param name      Name of the target memory I/O queue.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_open_pipe(_Out_ int *memqd_out, _In_z_ const char *name)
        ATTR_NONNULL(0, 1);

    /**
     * @brief Creates a socket I/O queue.
     *
     * @param sockqd_out Storage location for the socket I/O queue descriptor.
     * @param domain     Communication domain for the new socket.
     * @param type       Type of the socket.
     * @param protocol   Communication protocol for the new socket.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_socket(_Out_ int *sockqd_out, _In_ int domain, _In_ int type, _In_ int protocol)
        ATTR_NONNULL(0);

    /**
     * @brief Sets as passive a socket I/O queue.
     *
     * @param sockqd  I/O queue descriptor of the target socket.
     * @param backlog Maximum length for the queue of pending connections.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_listen(_In_ int sockqd, _In_ int backlog);

    /**
     * @brief Binds an address to a socket I/O queue.
     *
     * @param sockqd I/O queue descriptor of the target socket.
     * @param addr   Bind address.
     * @param size   Effective size of the socket address data structure.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_bind(_In_ int sockqd, _In_reads_bytes_(size) const struct sockaddr *addr, _In_ socklen_t size)
        ATTR_NONNULL(1);

    /**
     * @brief Asynchronously accepts a connection request on a socket I/O queue.
     *
     * @param qt_out Store location for I/O queue token.
     * @param sockqd I/O queue descriptor of the target socket.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_accept(_Out_ demi_qtoken_t *qt_out, _In_ int sockqd)
        ATTR_NONNULL(0);

    /**
     * @brief Asynchronously initiates a connection on a socket I/O queue.
     *
     * @param qt_out Store location for I/O queue token.
     * @param sockqd I/O queue descriptor of the target socket.
     * @param addr   Address of remote host.
     * @param size   Effective size of the socked address data structure.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_connect(_Out_ demi_qtoken_t *qt_out, _In_ int sockqd,
                            _In_reads_bytes_(size) const struct sockaddr *addr, _In_ socklen_t size)
        ATTR_NONNULL(0, 2);


    /**
     * @brief Closes an I/O queue descriptor.
     *
     * @param qd Target I/O queue descriptor.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_close(_In_ int qd);

    /**
     * @brief Asynchronously pushes a scatter-gather array to an I/O queue.
     *
     * @param qt_out Store location for I/O queue token.
     * @param qd     Target I/O queue descriptor.
     * @param sga    Scatter-gather array to push.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_push(_Out_ demi_qtoken_t *qt_out, _In_ int qd, _In_ const demi_sgarray_t *sga)
        ATTR_NONNULL(0, 2);

    /**
     * @brief Asynchronously pushes a scatter-gather array to a socket I/O queue.
     *
     * @param qt_out    Store location for I/O queue token.
     * @param sockqd    I/O queue descriptor of the target socket.
     * @param sga       Scatter-gather array to push.
     * @param dest_addr Address of destination host.
     * @param size      Effective size of the socked address data structure.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_pushto(_Out_ demi_qtoken_t *qt_out, _In_ int sockqd, _In_ const demi_sgarray_t *sga,
                           _In_reads_bytes_(size) const struct sockaddr *dest_addr, _In_ socklen_t size)
        ATTR_NONNULL(0, 2, 3);

    /**
     * @brief Asynchronously pops a scatter-gather array from an I/O queue.
     *
     * @param qt_out Store location for I/O queue token.
     * @param qd     Target I/O queue descriptor.
     *
     * @return On successful completion, zero is returned. On failure, a positive error code is returned instead.
     */
    extern int demi_pop(_Out_ demi_qtoken_t *qt_out, _In_ int qd)
        ATTR_NONNULL(0);

#ifdef __cplusplus
}
#endif

#endif /* DEMI_LIBOS_H_IS_INCLUDED */
