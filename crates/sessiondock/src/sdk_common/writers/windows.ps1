# Read-only SDK admission evidence. Never calls RmShutdown/RmRestart, writes
# process memory, reads environment blocks, or requests debug privileges.
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class SdkWriterEvidence {
    [DllImport("shell32.dll", SetLastError=true)] static extern IntPtr CommandLineToArgvW([MarshalAs(UnmanagedType.LPWStr)] string command, out int argc);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern bool ReadProcessMemory(IntPtr h, IntPtr address, byte[] buffer, UIntPtr size, out UIntPtr read);
    [DllImport("ntdll.dll")] static extern int NtQueryInformationProcess(IntPtr h, int kind, IntPtr buffer, int size, out int returned);
    public static string[] Args(string command) {
        if (String.IsNullOrEmpty(command)) return new string[0];
        int count; IntPtr ptr = CommandLineToArgvW(command, out count);
        if (ptr == IntPtr.Zero) return new string[0];
        try { var args = new string[count]; for (int i=0; i<count; ++i) args[i] = Marshal.PtrToStringUni(Marshal.ReadIntPtr(ptr,i*IntPtr.Size)); return args; }
        finally { LocalFree(ptr); }
    }
    static byte[] Read(IntPtr h, long addr, int size) {
        var bytes = new byte[size]; UIntPtr read;
        if (!ReadProcessMemory(h, new IntPtr(addr), bytes, (UIntPtr)size, out read) || read.ToUInt64() != (ulong)size) return null;
        return bytes;
    }
    static long Ptr(byte[] bytes, int offset, bool x86) { return x86 ? BitConverter.ToUInt32(bytes,offset) : BitConverter.ToInt64(bytes,offset); }
    // PEB/RTL_USER_PROCESS_PARAMETERS are OS-private layouts. Query only the
    // cwd UNICODE_STRING; fail unavailable on inaccessible/unsupported layouts.
    // Both native x64 and WOW64 x86 are handled from a 64-bit collector.
    public static string Cwd(uint pid) {
        if (IntPtr.Size != 8) return null;
        IntPtr h = OpenProcess(0x410, false, pid); // QUERY_INFORMATION | VM_READ
        if (h == IntPtr.Zero) return null;
        IntPtr info = Marshal.AllocHGlobal(48);
        try {
            int returned; bool x86 = false; long peb;
            if (NtQueryInformationProcess(h,26,info,8,out returned) == 0 && Marshal.ReadInt64(info) != 0) { peb = Marshal.ReadInt64(info); x86 = true; }
            else { if (NtQueryInformationProcess(h,0,info,48,out returned) != 0) return null; peb = Marshal.ReadInt64(info,8); }
            byte[] p = Read(h,peb+(x86 ? 0x10 : 0x20),x86 ? 4 : 8); if (p == null) return null;
            long parameters = Ptr(p,0,x86);
            byte[] dir = Read(h,parameters+(x86 ? 0x24 : 0x38),x86 ? 8 : 16); if (dir == null) return null;
            int length = BitConverter.ToUInt16(dir,0); int maximum = BitConverter.ToUInt16(dir,2);
            if (length == 0 || (length & 1) != 0 || length > maximum) return null;
            byte[] name = Read(h,Ptr(dir,x86 ? 4 : 8,x86),length);
            return name == null ? null : Encoding.Unicode.GetString(name);
        } finally { Marshal.FreeHGlobal(info); CloseHandle(h); }
    }
    [StructLayout(LayoutKind.Sequential)] public struct FileTime { public uint low, high; }
    [StructLayout(LayoutKind.Sequential)] public struct UniqueProcess { public uint pid; public FileTime start; }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct ProcessInfo {
        public UniqueProcess process;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=256)] public string name;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=64)] public string service;
        public uint type, status, session;
        [MarshalAs(UnmanagedType.Bool)] public bool restartable;
    }
    [DllImport("rstrtmgr.dll", CharSet=CharSet.Unicode)] static extern int RmStartSession(out uint session, uint flags, StringBuilder key);
    [DllImport("rstrtmgr.dll", CharSet=CharSet.Unicode)] static extern int RmRegisterResources(uint session, uint count, string[] paths, uint applications, IntPtr apps, uint services, IntPtr names);
    [DllImport("rstrtmgr.dll")] static extern int RmGetList(uint session, out uint needed, ref uint count, [In,Out] ProcessInfo[] rows, ref uint reasons);
    [DllImport("rstrtmgr.dll")] static extern int RmEndSession(uint session);
    public static UniqueProcess[] Holders(string path) {
        uint session; int status = RmStartSession(out session,0,new StringBuilder(33));
        if (status != 0) throw new InvalidOperationException("SDK file observation start: " + status);
        try {
            status = RmRegisterResources(session,1,new string[]{path},0,IntPtr.Zero,0,IntPtr.Zero);
            if (status != 0) throw new InvalidOperationException("SDK file observation register: " + status);
            uint count=0, needed=0, reasons=0; ProcessInfo[] rows = null;
            while (true) {
                status = RmGetList(session,out needed,ref count,rows,ref reasons);
                if (status == 0) { var result = new UniqueProcess[count]; for (int i=0; i<count; ++i) result[i]=rows[i].process; return result; }
                if (status != 234) throw new InvalidOperationException("SDK file observation list: " + status);
                count=needed; rows=new ProcessInfo[count];
            }
        } finally { RmEndSession(session); }
    }
}
'@
# Keep all parent edges, including non-CLI wrappers. Same-user candidates only;
# command lines and cwd of other users are never requested by this collector.
$me = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$rows = [System.Collections.Generic.List[object]]::new()
foreach ($p in Get-CimInstance Win32_Process) {
    $args = @(); $cwd = $null; $started = 0.0
    if ($p.Name -notmatch '^(claude.*|codebuddy.*|cbc|kiro.*|opencode.*|agy.*|node|bun)(\.exe)?$') {
        $rows.Add(@{pid=[uint32]$p.ProcessId; ppid=[uint32]$p.ParentProcessId; args=@(); cwd=$null; files=[System.Collections.Generic.List[string]]::new(); started=$(if ($p.CreationDate) { ([DateTimeOffset]$p.CreationDate).ToUnixTimeMilliseconds()/1000.0 } else { 0.0 })})
        continue
    }
    $owner = Invoke-CimMethod -InputObject $p -MethodName GetOwnerSid -ErrorAction SilentlyContinue
    if ($owner.Sid -eq $me) {
        $args = [SdkWriterEvidence]::Args($p.CommandLine)
        $cwd = [SdkWriterEvidence]::Cwd($p.ProcessId)
        if ($p.CreationDate) { $started = ([DateTimeOffset]$p.CreationDate).ToUnixTimeMilliseconds()/1000.0 }
        try {
            $live = [System.Diagnostics.Process]::GetProcessById($p.ProcessId)
            try { $observed = ([DateTimeOffset]$live.StartTime).ToUnixTimeMilliseconds()/1000.0 } finally { $live.Dispose() }
            if ([Math]::Abs($observed-$started) -gt 0.002) { continue }
        } catch { continue }
    }
    $rows.Add(@{pid=[uint32]$p.ProcessId; ppid=[uint32]$p.ParentProcessId; args=@($args); cwd=$cwd; files=[System.Collections.Generic.List[string]]::new(); started=$started})
}
$paths = @()
if ($env:SESSIONDOCK_WRITER_FILES) { $paths = @($env:SESSIONDOCK_WRITER_FILES | ConvertFrom-Json) }
foreach ($path in ($paths | Select-Object -Unique)) {
    if (!(Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    foreach ($holder in [SdkWriterEvidence]::Holders($path)) {
        $start = ([double]$holder.start.high * 4294967296 + $holder.start.low)/10000000 - 11644473600
        foreach ($row in $rows) {
            # PID reuse cannot attach a previous owner's file evidence.
            if ($row.pid -eq $holder.pid -and [Math]::Abs($row.started-$start) -lt 0.002) { $row.files.Add($path) }
        }
    }
}
ConvertTo-Json -InputObject @($rows.ToArray()) -Depth 5 -Compress
