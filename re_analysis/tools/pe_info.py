import sys, math, pefile, hashlib

path = sys.argv[1]
pe = pefile.PE(path, fast_load=False)
print("Machine:", hex(pe.FILE_HEADER.Machine))
print("TimeDateStamp:", pe.FILE_HEADER.TimeDateStamp)
print("Characteristics:", hex(pe.FILE_HEADER.Characteristics))
print("Subsystem:", pe.OPTIONAL_HEADER.Subsystem, "DllCharacteristics:", hex(pe.OPTIONAL_HEADER.DllCharacteristics))
print("ImageBase:", hex(pe.OPTIONAL_HEADER.ImageBase), "EntryPoint:", hex(pe.OPTIONAL_HEADER.AddressOfEntryPoint))
print("SizeOfImage:", hex(pe.OPTIONAL_HEADER.SizeOfImage))
print("Checksum:", hex(pe.OPTIONAL_HEADER.CheckSum))
print()

def ent(data):
    if not data: return 0.0
    from collections import Counter
    c = Counter(data); n = len(data)
    return -sum((v/n)*math.log2(v/n) for v in c.values())

print(f"{'Section':10} {'VAddr':>10} {'VSize':>9} {'RawOff':>10} {'RawSize':>9} {'Entropy':>8}  Flags")
for s in pe.sections:
    name = s.Name.rstrip(b'\x00').decode('latin1')
    data = s.get_data()
    print(f"{name:10} {s.VirtualAddress:>10x} {s.Misc_VirtualSize:>9x} {s.PointerToRawData:>10x} {s.SizeOfRawData:>9x} {ent(data):>8.3f}  {s.Characteristics:#010x}")

# Overlay
last = max(s.PointerToRawData + s.SizeOfRawData for s in pe.sections)
import os
sz = os.path.getsize(path)
print("\nFile size:", sz, "End of sections:", last, "Overlay:", sz-last)

print("\nData directories:")
for i, d in enumerate(pe.OPTIONAL_HEADER.DATA_DIRECTORY):
    if d.VirtualAddress or d.Size:
        print(f"  {i:2} {d.name:24} RVA={d.VirtualAddress:#x} Size={d.Size:#x}")

print("\nImports:")
if hasattr(pe, 'DIRECTORY_ENTRY_IMPORT'):
    for e in pe.DIRECTORY_ENTRY_IMPORT:
        dll = e.dll.decode()
        fns = [ (i.name.decode() if i.name else f"ord{i.ordinal}") for i in e.imports ]
        print(f"  {dll}: {', '.join(fns)}")
else:
    print("  none")

if hasattr(pe, 'DIRECTORY_ENTRY_EXPORT'):
    print("\nExports:", [x.name.decode() for x in pe.DIRECTORY_ENTRY_EXPORT.symbols if x.name])
else:
    print("\nExports: none")

print("\nDebug dir:", pe.OPTIONAL_HEADER.DATA_DIRECTORY[6].VirtualAddress, pe.OPTIONAL_HEADER.DATA_DIRECTORY[6].Size)
if hasattr(pe, 'DIRECTORY_ENTRY_DEBUG'):
    for d in pe.DIRECTORY_ENTRY_DEBUG:
        print("  debug type", d.struct.Type, "size", d.struct.SizeOfData, "addr", hex(d.struct.AddressOfRawData))
        try:
            print("   pdb:", d.entry.PdbFileName.decode('latin1', 'replace').rstrip('\x00'))
        except Exception:
            pass
