Unicode true
Name "RustZIP read-only test"
OutFile "${OUTPUT_FILE}"
RequestExecutionLevel user
!ifndef COMPRESSION
  !define COMPRESSION zlib
!endif
!ifdef SOLID
  SetCompressor /SOLID /FINAL ${COMPRESSION}
!else
  SetCompressor /FINAL ${COMPRESSION}
!endif
Section
  SetOutPath "$INSTDIR"
  File "payload.txt"
  File "empty.txt"
!ifdef LARGE_SOURCE
  File /oname=compressed.txt "${LARGE_SOURCE}"
!endif
  SetOutPath "$INSTDIR\目录"
  File /oname=中文.txt "payload.txt"
SectionEnd
