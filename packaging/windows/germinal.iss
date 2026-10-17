#ifndef AppVersion
  #error AppVersion must be provided by the packaging script.
#endif
#ifndef PayloadDir
  #error PayloadDir must be provided by the packaging script.
#endif

[Setup]
AppId={{48D47179-3115-4FE8-8273-D908BDE05598}
AppName=Germinal
AppVersion={#AppVersion}
AppPublisher=cradiy
DefaultDirName={localappdata}\Programs\Germinal
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
UninstallDisplayIcon={app}\germinal.exe
LicenseFile={#PayloadDir}\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Files]
Source: "{#PayloadDir}\germinal.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Germinal"; Filename: "{app}\germinal.exe"

[Run]
Filename: "{app}\germinal.exe"; Description: "Launch Germinal"; Flags: nowait postinstall skipifsilent
