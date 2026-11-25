# Writes a stand-in for a camera raw file: a TIFF header, bytes that stand for sensor data
# (including a false start of a JPEG), a small preview and a large one. Cameras lay their
# previews out like this; the reader looks for the largest whole JPEG, not at the TIFF tags.
import sys, os
small, large, out = sys.argv[1:4]
data = b"II*\x00\x08\x00\x00\x00" + os.urandom(4096).replace(b"\xff\xd8\xff", b"\x00\x00\x00") + b"\xff\xd8\xff\x00"
data += open(small, "rb").read() + os.urandom(2048).replace(b"\xff\xd8\xff", b"\x00\x00\x00") + open(large, "rb").read() + b"\x00" * 64
open(out, "wb").write(data)
