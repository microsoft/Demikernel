﻿using Demikernel.Interop;
using System.Net;
using System.Net.Sockets;
using Socket = Demikernel.Interop.Socket;

namespace Demikernel.Test;

public class EchoServer : MessagePump<int>
{
    public const int Port = 12314;

    public static EndPoint EndPoint { get; } = new IPEndPoint(IPAddress.Loopback, Port);

    public EchoServer()
    {
        if (!TestBase.LibraryAvailable) return;
        Start();
    }

    protected override void OnStart()
    {
        Accept(42, AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp, EndPoint, 32);
    }

    protected override bool OnPop(Socket socket, ref int state, in ScatterGatherArray payload)
    {
        if (payload.IsEmpty)
        {
            Close(socket);
            return false;
        }
        else
        {
            Push(socket, state, in payload); // note that this calls sgafree correctly
            return true; // do another pop
        }
    }
}