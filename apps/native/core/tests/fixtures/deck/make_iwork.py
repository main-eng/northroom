# Writes stand-ins for a Keynote file laid out as Keynote saves it since 2013: a zip of
# undocumented Index/*.iwa parts and the previews the reader shows. And one from before 2013,
# which keeps its preview under QuickLook/.
import sys, zipfile
preview, small, out_new, out_old = sys.argv[1:5]
with zipfile.ZipFile(out_new, "w") as package:
    package.writestr("Index/Document.iwa", b"\x00\x01undocumented")
    package.writestr("Metadata/Properties.plist", b"<plist/>")
    package.write(small, "preview-micro.jpg")
    package.write(preview, "preview.jpg")
with zipfile.ZipFile(out_old, "w") as package:
    package.writestr("index.apxl.gz", b"\x1f\x8b")
    package.write(small, "QuickLook/Thumbnail.jpg")
