"""Bump patch version in Cargo.toml, build release, and copy exe."""
import re, subprocess, sys, os

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

# Build
print("Building release...")
r = subprocess.run(["cargo", "build", "--release"], capture_output=True, text=True)
if r.returncode != 0:
    print("BUILD FAILED:")
    print(r.stderr)
    sys.exit(1)

# Copy exe
import shutil
src = os.path.join("target", "release", "md-viewer.exe")
dst = "md-viewer.exe"
shutil.copy2(src, dst)

size = os.path.getsize(dst) / 1024 / 1024
print(f"Done! md-viewer.exe v{new_version} ({size:.1f} MB)")
