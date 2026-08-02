import struct, sys, re, collections

LUMP_NAMES = {0:"ENTITIES",1:"PLANES",2:"TEXDATA",3:"VERTEXES",7:"FACES",8:"LIGHTING",10:"LEAFS",12:"EDGES",13:"SURFEDGES",14:"MODELS",18:"BRUSHES",19:"BRUSHSIDES",26:"DISPINFO",33:"DISP_VERTS",35:"GAME_LUMP",40:"PAKFILE",43:"TEXDATA_STRING_DATA",44:"TEXDATA_STRING_TABLE",48:"DISP_TRIS",53:"LIGHTING_HDR"}

def parse(path):
    f = open(path,'rb')
    ident, version = struct.unpack('<ii', f.read(8))
    print(f"\n##### {path}")
    print(f"ident={ident:#x} ({'VBSP' if ident==0x50534256 else '???'}), version={version}")
    lumps = []
    for i in range(64):
        ofs, ln, ver, fourcc = struct.unpack('<iii4s', f.read(16))
        lumps.append((ofs, ln, ver, fourcc))
    maprev, = struct.unpack('<i', f.read(4))
    print(f"mapRevision={maprev}")
    def lump(i): 
        ofs, ln, ver, fourcc = lumps[i]
        f.seek(ofs); return f.read(ln)
    # sizes/counts
    sizes = {1:20, 7:56, 14:48, 18:12, 19:8, 26:176, 33:20, 3:12, 12:4, 13:4, 2:32, 6:72, 48:2}
    for i in (1,3,12,13,7,2,14,18,19,26,33,48,8,53,40):
        ofs, ln, ver, fourcc = lumps[i]
        cnt = f" count={ln//sizes[i]}" if i in sizes and ln else ""
        print(f"lump {i:2d} {LUMP_NAMES.get(i,''):22s} len={ln:9d} ver={ver}{cnt}")
    # leaf version
    print(f"lump 10 LEAFS len={lumps[10][1]} ver={lumps[10][2]} (32B/leaf => {lumps[10][1]/32:.1f}, 56B/leaf => {lumps[10][1]/56:.1f})")
    # game lump
    gl_ofs, gl_len, _, _ = lumps[35]
    f.seek(gl_ofs)
    n, = struct.unpack('<i', f.read(4))
    for _ in range(n):
        gid, flags, ver, ofs, ln = struct.unpack('<iHHii', f.read(16))
        tag = struct.pack('<i', gid)[::-1].decode('ascii', 'replace')
        print(f"  gamelump id={tag!r} flags={flags} version={ver} len={ln}")
        if tag == 'sprp' and ln > 0:
            pos = f.tell(); f.seek(ofs)
            cnt, = struct.unpack('<i', f.read(4))
            print(f"    static prop dict entries={cnt}")
            f.seek(pos)
    # entities
    ent = lump(0).decode('latin-1', 'replace')
    classes = collections.Counter(re.findall(r'"classname"\s+"([^"]+)"', ent))
    print("  entity classnames (top):")
    for c, k in classes.most_common(200):
        print(f"    {k:5d} {c}")
    # brush contents histogram
    data = lump(18)
    contents = collections.Counter()
    for j in range(len(data)//12):
        fs, ns, ct = struct.unpack_from('<iii', data, j*12)
        contents[ct] += 1
    print("  brush contents flags histogram:")
    for ct, k in contents.most_common(12):
        print(f"    {k:5d} brushes contents={ct:#010x}")
    # texdata string sample
    tds = lump(43).split(b'\0')
    names = [t.decode('latin-1') for t in tds if t]
    print(f"  texdata strings: {len(names)}; sample: {names[:12]}")
    # pakfile: count files by extension
    pak = lump(40)
    exts = collections.Counter(m.group(1).lower() for m in re.finditer(rb'\.([A-Za-z0-9]{2,4})PK', b'')) # placeholder
    import io, zipfile
    try:
        z = zipfile.ZipFile(io.BytesIO(pak))
        exts = collections.Counter(nm.rsplit('.',1)[-1].lower() for nm in z.namelist() if '.' in nm)
        print(f"  pakfile: {len(z.namelist())} files, by ext: {dict(exts.most_common(10))}")
    except Exception as e:
        print(f"  pakfile: unreadable as zip ({e})")
    # worldspawn skyname
    m = re.search(r'"skyname"\s+"([^"]+)"', ent)
    if m: print(f"  skyname={m.group(1)}")

for p in sys.argv[1:]:
    parse(p)
