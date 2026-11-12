#!/usr/bin/env python3
# machobind.py PROGRAM [NAME...]: which library each symbol a 64-bit Mach-O program imports is bound to (all of them,
# or those whose name contains a NAME). "FLAT" means the system searches every loaded library for it when the program
# runs, which is what the old-Mac build needs: see docs/MACOS_SNOW_LEOPARD.md.
import struct,sys
d=open(sys.argv[1],'rb').read()
magic,cpu,sub,ft,ncmds,szc,flags,_=struct.unpack_from('<IiiIIIII',d,0)
off=32; dylibs=[]; info=None
for _ in range(ncmds):
    cmd,sz=struct.unpack_from('<II',d,off)
    if cmd in (0xc,0x8000001f,0x80000018): # LOAD_DYLIB, REEXPORT, WEAK
        no=struct.unpack_from('<I',d,off+8)[0]; dylibs.append(d[off+no:off+sz].split(b'\0')[0].decode())
    if cmd in (0x22,0x80000022): info=struct.unpack_from('<10I',d,off+8)
    off+=sz
print("flags: twolevel" if flags&0x80 else "flags: FLAT", hex(flags))
def uleb(b,i):
    r=0;s=0
    while True:
        c=b[i];i+=1;r|=(c&0x7f)<<s;s+=7
        if not c&0x80: return r,i
def walk(o,n,kind):
    b=d[o:o+n]; i=0; lib=0; out=[]
    while i<len(b):
        op=b[i]&0xf0; imm=b[i]&0x0f; i+=1
        if op==0x10: lib=imm
        elif op==0x20: lib,i=uleb(b,i)
        elif op==0x30: lib=-(16-imm) if imm else 0
        elif op==0x40:
            e=b.index(0,i); out.append((b[i:e].decode(),lib,kind)); i=e+1
        elif op in (0x60,0x80,0xa0): _,i=uleb(b,i)
        elif op==0x70: _,i=uleb(b,i)
        elif op==0xc0: _,i=uleb(b,i); _,i=uleb(b,i)
    return out
if info is None:
    # a program linked for 10.5 has no bind tables (they arrived with 10.6); its imports are in the symbol table
    print("(no bind tables: linked for a system older than 10.6; use nm -u for the import list)")
    sys.exit(0)
syms=walk(info[2],info[3],'bind')+walk(info[6],info[7],'lazy')
want=sys.argv[2:]
for name,lib,kind in sorted(set(syms)):
    libn=dylibs[lib-1].split("/")[-1] if lib>0 else ("FLAT" if lib==-2 else str(lib))
    if not want or any(w in name for w in want): print(kind,name,"->",libn)
