"""Bump patch version in Cargo.toml, build release exe, and build Windows installer."""
import re, subprocess, sys, os, shutil

os.chdir(os.path.dirname(os.path.abspath(__file__)))

# Read Cargo.toml
with open("Cargo.toml", "r") as f:
    content = f.read()

# Find and bump version
m = re.search(r'version\s*=\s*"(\d+)\.(\d+)\.(\d+)"', content)
if not m:
    print("ERROR: version not found in Cargo.toml")
    sys.exit(1)

major, minor, patch = int(m.group(1)), int(m.group(2)), int(m.group(3))
new_version = f"{major}.{minor}.{patch + 1}"

content = content[:m.start()] + f'version = "{new_version}"' + content[m.end():]
with open("Cargo.toml", "w") as f:
    f.write(content)

print(f"Version: {m.group(1)}.{m.group(2)}.{m.group(3)} -> {new_version}")

# Build release exe
print("Building release exe...")
r = subprocess.run(["cargo", "build", "--release"], capture_output=True, text=True)
if r.returncode != 0:
    print("BUILD FAILED:")
    print(r.stderr)
    sys.exit(1)

# Copy exe to project root (installer.iss picks it up from here)
src_exe = os.path.join("target", "release", "md-viewer.exe")
dst_exe = "md-viewer.exe"
shutil.copy2(src_exe, dst_exe)
exe_size = os.path.getsize(dst_exe) / 1024 / 1024
print(f"Done: md-viewer.exe v{new_version} ({exe_size:.1f} MB)")

# Locate Inno Setup compiler (ISCC.exe)
iscc_candidates = [
    os.path.expandvars(r"%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"),
    r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    r"C:\Program Files\Inno Setup 6\ISCC.exe",
]
iscc = next((p for p in iscc_candidates if os.path.exists(p)), None)

if iscc is None:
    print("\n[WARN] Inno Setup 6 not found, skipping installer build.")
    print("       Install from: https://jrsoftware.org/isdl.php")
    sys.exit(0)

# Build installer
print(f"\nBuilding installer with: {iscc}")
os.makedirs("dist", exist_ok=True)
r = subprocess.run(
    [iscc, f"/DMyAppVersion={new_version}", "installer.iss"],
    capture_output=True, text=True
)
if r.returncode != 0:
    print("INSTALLER BUILD FAILED:")
    print(r.stdout)
    print(r.stderr)
    sys.exit(1)

installer_path = os.path.join("dist", f"md-viewer-setup-v{new_version}.exe")
if os.path.exists(installer_path):
    isize = os.path.getsize(installer_path) / 1024 / 1024
    print(f"Installer: {installer_path} ({isize:.1f} MB)")
else:
    print(f"[WARN] Installer output not found at expected path: {installer_path}")
