#!/usr/bin/env dotnet-script
#r "nuget: SimpleExec, 7.0.0"
#r "nuget: Mono.Posix.NETStandard, 5.20.1-preview"

using static SimpleExec.Command;
using static System.Console;
using System.Threading;
using System.Threading.Tasks;
using Mono.Unix;
using Mono.Unix.Native;
using System.Collections.Specialized;
using System;

static var apps = new List<(string, string)> {
   ("cross","build --target arm-unknown-linux-gnueabihf"),
   ("scp",$"./target/arm-unknown-linux-gnueabihf/debug/rad_io {Environment.GetEnvironmentVariable("TARGET")}:./"),
   ("scp",$"./contrib/radIO.service {Environment.GetEnvironmentVariable("TARGET")}:./"),
 };

static var tasks = new Dictionary<string, Task>();

static var signals = new UnixSignal[] {
                new UnixSignal(Signum.SIGINT),
                new UnixSignal(Signum.SIGTERM)
            };





foreach (var app in apps)
{
    var (k, v) = app;
    WriteLine($"Starting task: {k}");
    await RunAsync(k, v);
    WriteLine("\tOK!");

}

