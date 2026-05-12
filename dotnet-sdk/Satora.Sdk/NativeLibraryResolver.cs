using System;
using System.IO;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Satora.Sdk;

/// <summary>
/// Hooks <see cref="NativeLibrary.SetDllImportResolver"/> so the
/// uniffi-generated P/Invokes find <c>satora_sdk_ffi</c> from
/// <c>runtimes/&lt;rid&gt;/native/</c> relative to this assembly's own
/// location.
///
/// Why: when this SDK is loaded by a host that uses a custom
/// <see cref="System.Runtime.Loader.AssemblyLoadContext"/> (e.g. BTCPay
/// Server's plugin loader), the host doesn't honor the consumer
/// assembly's deps.json for native lib resolution — it only searches
/// flat in the assembly's directory. This resolver makes the
/// standard RID-scoped layout work in those hosts.
///
/// <see cref="ModuleInitializerAttribute"/> guarantees this runs before
/// any code in the assembly, including the static cctor of
/// <c>_UniFFILib</c> which invokes the first DllImport.
/// </summary>
internal static class NativeLibraryResolver
{
    private const string NativeLibraryName = "satora_sdk_ffi";

    // CA2255 flags ModuleInitializer in library code as unusual; here
    // it's the whole point — register the DllImport resolver before
    // any P/Invoke runs, with no consumer involvement.
#pragma warning disable CA2255
    [ModuleInitializer]
#pragma warning restore CA2255
    internal static void Init()
    {
        NativeLibrary.SetDllImportResolver(
            typeof(NativeLibraryResolver).Assembly,
            Resolve);
    }

    private static IntPtr Resolve(
        string libraryName,
        Assembly assembly,
        DllImportSearchPath? searchPath)
    {
        if (libraryName != NativeLibraryName)
            return IntPtr.Zero;

        var asmDir = Path.GetDirectoryName(assembly.Location);
        if (string.IsNullOrEmpty(asmDir))
            return IntPtr.Zero;

        var fileName = GetNativeFileName();
        var rid = RuntimeInformation.RuntimeIdentifier;

        // 1) Exact-RID match — covers the standard portable publish
        //    case (`linux-x64`, `osx-arm64`, …) and anything we
        //    explicitly packaged.
        if (TryLoadFromRid(asmDir, rid, fileName, out var loaded))
            return loaded;

        // 2) Portable-RID fallback. On some hosts `RuntimeIdentifier`
        //    returns a distro-specific value (`ubuntu.22.04-x64`,
        //    `osx.13-arm64`, etc.) that doesn't match the portable RIDs
        //    we package. Map (OS, arch) → portable RID and retry.
        //
        //    Skip the fallback when the runtime is musl: we don't ship
        //    musl binaries, and silently loading a glibc binary into a
        //    musl process would fail at link time (or worse, crash).
        if (!rid.Contains("musl", StringComparison.Ordinal))
        {
            var portable = TryPortableRid();
            if (portable is not null
                && !string.Equals(portable, rid, StringComparison.Ordinal)
                && TryLoadFromRid(asmDir, portable, fileName, out loaded))
            {
                return loaded;
            }
        }

        // 3) Flat alongside the assembly — legacy fallback for
        //    consumers that opted into <SatoraSdkFlattenNativeLibs>true.
        var flatPath = Path.Combine(asmDir, fileName);
        if (File.Exists(flatPath))
            return NativeLibrary.Load(flatPath);

        // 4) Give up — default loader will try the OS lookup paths
        //    (LD_LIBRARY_PATH / PATH / DYLD_LIBRARY_PATH).
        return IntPtr.Zero;
    }

    private static bool TryLoadFromRid(
        string asmDir,
        string rid,
        string fileName,
        out IntPtr handle)
    {
        var path = Path.Combine(asmDir, "runtimes", rid, "native", fileName);
        if (File.Exists(path))
        {
            handle = NativeLibrary.Load(path);
            return true;
        }
        handle = IntPtr.Zero;
        return false;
    }

    private static string? TryPortableRid()
    {
        var arch = RuntimeInformation.OSArchitecture switch
        {
            Architecture.X64 => "x64",
            Architecture.Arm64 => "arm64",
            _ => null,
        };
        if (arch is null)
            return null;

        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            return $"win-{arch}";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
            return $"osx-{arch}";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Linux))
            return $"linux-{arch}";
        return null;
    }

    private static string GetNativeFileName()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
            return "satora_sdk_ffi.dll";
        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
            return "libsatora_sdk_ffi.dylib";
        return "libsatora_sdk_ffi.so";
    }
}
