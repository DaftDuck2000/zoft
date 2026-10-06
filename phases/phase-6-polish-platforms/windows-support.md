# Feature: Windows Support

## Phase: 6 — Polish & Platform Expansion
## ID: 6.2
## Priority: Low
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 5.1

## Description
Native Windows support: WASAPI (exclusive/shared), VST3, installer (MSIX/NSIS), code signing, HiDPI, dark mode.

## Requirements

- [ ] WASAPI backend: Exclusive mode for low latency, shared for compatibility
- [ ] VST3 hosting: COM initialization, standard paths, 32-bit bridge
- [ ] Installer: MSIX (Store) + NSIS (standalone), auto-update
- [ ] Code signing: EV certificate, timestamp, SmartScreen reputation
- [ ] HiDPI: Per-monitor DPI awareness, scaling
- [ ] Dark mode: System theme detection, title bar integration
- [ ] File associations: `.zoft` registry, open with
- [ ] MIDI: `midir` Windows backend (WinMM/UWP)
- [ ] Shell integration: Jump list, taskbar progress
- [ ] ARM64: Native Windows on ARM build

## Technical Details

### WASAPI Backend
```rust
// Use `wasapi` crate for exclusive/shared mode

struct WasapiEngine {
    client: wasapi::AudioClient,
    render_client: wasapi::AudioRenderClient,
    format: wasapi::WaveFormat,
    buffer_size: u32,
    event_handle: HANDLE,           // For exclusive mode
}

impl WasapiEngine {
    fn start_exclusive(&mut self) -> Result<(), WasapiError> {
        self.client.initialize(
            wasapi::AUDCLNT_SHAREMODE_EXCLUSIVE,
            wasapi::AUDCLNT_STREAMFLAGS_EVENTCALLBACK | wasapi::AUDCLNT_STREAMFLAGS_NOPERSIST,
            self.buffer_size as i64 * 10000, // 100ns units
            0,
            &self.format,
            std::ptr::null_mut(),
        )?;
        
        self.event_handle = self.client.set_event_handle()?;
        self.render_client = self.client.get_service()?;
        self.client.start()?;
        Ok(())
    }
}
```

### VST3 on Windows
```rust
// COM initialization required
use winapi::um::combaseapi::CoInitializeEx;
use winapi::um::objbase::COINIT_MULTITHREADED;

fn init_com() -> Result<(), ComError> {
    unsafe {
        let hr = CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED);
        if hr == S_OK || hr == S_FALSE { Ok(()) } else { Err(ComError(hr)) }
    }
}

// VST3 SDK paths
const VST3_PATHS: &[&str] = &[
    r"C:\Program Files\Common Files\VST3",
    r"C:\Program Files (x86)\Common Files\VST3",
    r"C:\Users\Public\Documents\VST3",
];
```

### Installer (NSIS)
```nsis
; installer.nsi
Name "Zoft"
OutFile "Zoft_Setup.exe"
InstallDir "$PROGRAMFILES\Zoft"
RequestExecutionLevel admin

Section "Main"
    SetOutPath "$INSTDIR"
    File /r "target\release\zoft.exe"
    File /r "assets\*"
    
    ; Start menu
    CreateDirectory "$SMPROGRAMS\Zoft"
    CreateShortcut "$SMPROGRAMS\Zoft\Zoft.lnk" "$INSTDIR\zoft.exe"
    
    ; File association
    WriteRegStr HKCR ".zoft" "" "ZoftProject"
    WriteRegStr HKCR "ZoftProject" "" "Zoft Project"
    WriteRegStr HKCR "ZoftProject\shell\open\command" "" '"$INSTDIR\zoft.exe" "%1"'
    
    ; Uninstaller
    WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
    Delete "$INSTDIR\*.*"
    RMDir /r "$INSTDIR"
    DeleteRegKey HKCR ".zoft"
    DeleteRegKey HKCR "ZoftProject"
SectionEnd
```

### Code Signing
```powershell
# sign.ps1
$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object { $_.Subject -like "*Zoft*" } | Select-Object -First 1
Set-AuthenticodeSignature -FilePath "zoft.exe" -Certificate $cert -TimestampServer "http://timestamp.digicert.com"
Set-AuthenticodeSignature -FilePath "Zoft_Setup.exe" -Certificate $cert -TimestampServer "http://timestamp.digicert.com"
```

### HiDPI Awareness
```rust
// In main() before GUI init
use winapi::um::shellscalingapi::SetProcessDpiAwarenessContext;
use winapi::um::winuser::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;

unsafe {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
}
```

## Acceptance Criteria

- [ ] WASAPI exclusive: < 3ms latency at 48kHz/128
- [ ] VST3 plugins load (FabFilter, etc.)
- [ ] NSIS installer creates shortcuts, file associations
- [ ] Signed: no SmartScreen warning
- [ ] HiDPI: sharp on 150%/200% scaling
- [ ] Dark mode: follows system theme
- [ ] ARM64 build runs on Snapdragon
- [ ] MIDI I/O works

## Progress Log

- YYYY-MM-DD: WASAPI backend
- YYYY-MM-DD: VST3 COM init + paths
- YYYY-MM-DD: NSIS installer
- YYYY-MM-DD: Code signing pipeline
- YYYY-MM-DD: HiDPI + dark mode
- YYYY-MM-DD: File associations
- YYYY-MM-DD: ARM64 build