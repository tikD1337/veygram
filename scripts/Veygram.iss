[Setup]
AppId={{863FC656-589A-4035-AB18-352FF660BD9A}
AppName=Veygram
AppVersion={#AppVersion}
AppPublisher=Veygram
AppPublisherURL=https://github.com/tikD1337/veygram
AppSupportURL=https://github.com/tikD1337/veygram/issues
AppUpdatesURL=https://github.com/tikD1337/veygram/releases
DefaultDirName={localappdata}\Programs\Veygram
DefaultGroupName=Veygram
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\veygram.exe
PrivilegesRequired=lowest
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
SetupIconFile={#SourceDir}\crates\app\assets\veygram.ico
VersionInfoVersion={#AppVersion}.0
OutputDir={#OutputDir}
OutputBaseFilename=Veygram-Setup-{#AppVersion}-x64
Compression=lzma2
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Files]
Source: "{#StageDir}\veygram.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\licenses\*.txt"; DestDir: "{app}\licenses"; Flags: ignoreversion
Source: "{#StageDir}\README.txt"; DestDir: "{app}"; DestName: "README.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\Veygram"; Filename: "{app}\veygram.exe"; WorkingDir: "{app}"

[Run]
Filename: "{app}\veygram.exe"; Description: "{cm:LaunchProgram,Veygram}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent runasoriginaluser
