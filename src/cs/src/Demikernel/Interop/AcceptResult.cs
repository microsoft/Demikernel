﻿using Demikernel.Internal;
using System.Diagnostics.CodeAnalysis;
using System.Runtime.InteropServices;

namespace Demikernel.Interop;

/// <summary>
/// Represents a client connection accepted from a listening server socket
/// </summary>
/// <remarks>The choice of public fields here is intentional, as this type is a raw P/Invoke layer</remarks>
[StructLayout(LayoutKind.Explicit, Pack = 1, Size = 20)]
public readonly struct AcceptResult
{
    private const int AddressLength = 16;

    /// <summary>
    /// Gets the queue (socket) associated with this result
    /// </summary>
    [FieldOffset(0)]
    public readonly int Qd;

    [FieldOffset(4)]
    private readonly byte _saddrStart; // [Sizes.SOCKET_ADDRESS];

    /// <summary>
    /// Gets this result as a <see cref="Socket"/>
    /// </summary>
    public Socket AsSocket() => new Socket(Qd);

    /// <summary>
    /// Gets this result as a <see cref="Stream"/>
    /// </summary>
    public Stream AsStream(bool ownsSocket = true)
        => new DemiKernelStream(AsSocket(), ownsSocket);

    /// <summary>
    /// Copies the address held in this result to the provided target
    /// </summary>
    /// <param name="destination"></param>
    public unsafe void CopyAddressTo(Span<byte> destination)
    {
        fixed (byte* ptr = &_saddrStart)
        {
            new Span<byte>(ptr, AddressLength).CopyTo(destination);
        }
    }

    /// <inheritdoc/>
    public override int GetHashCode() => Qd;

    /// <inheritdoc/>
    public override bool Equals([NotNullWhen(true)] object? obj)
        => obj is AcceptResult other && other.Qd == Qd;

    /// <inheritdoc/>
    public override unsafe string ToString()
    {
        var c = stackalloc char[AddressLength * 2];
        int offset = 0;
        fixed (byte* ptr = &_saddrStart)
        {
            string Hex = "0123456789abcdef";
            for (int i = 0; i < AddressLength; i++)
            {
                c[offset++] = Hex[ptr[i] & 0x0F];
                c[offset++] = Hex[ptr[i] >> 4];
            }
        }
        return new string(c, 0, AddressLength * 2);
    }
}