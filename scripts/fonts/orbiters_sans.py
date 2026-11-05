# Takes fontmake's variable Selawik to the interface's font: renamed (Selawik is an OFL Reserved Font Name, so a built
# version may not carry it) with Segoe UI's vertical metrics, so lines are as tall as on Windows.
import sys
from fontTools.ttLib import TTFont
src, dst = sys.argv[1:3]
f = TTFont(src)
name = f['name']
FAMILY = 'Orbiters Sans'
for rec in list(name.names):
    s = rec.toUnicode()
    if rec.nameID in (1, 3, 4, 6, 16, 21, 25) or 'Selawik' in s:
        if rec.nameID in (0, 7, 13):      # copyright, trademark, licence: copyright stays as the authors wrote it
            continue
        s2 = s.replace('Selawik-VF', 'OrbitersSans').replace('Selawik VF', FAMILY).replace('Selawik', FAMILY)
        if rec.nameID == 6: s2 = s2.replace(' ', '')
        rec.string = s2
name.setName('Orbiters Sans is a modified version of Selawik by Microsoft, built as a variable font with Segoe UI\'s vertical metrics.', 10, 3, 1, 0x409)
# Trademark line names Selawik: not this font's name now.
name.names = [r for r in name.names if r.nameID != 7]
hhea, os2 = f['hhea'], f['OS/2']
hhea.ascent, hhea.descent, hhea.lineGap = 2210, -514, 0
os2.sTypoAscender, os2.sTypoDescender, os2.sTypoLineGap = 2210, -514, 0
os2.usWinAscent, os2.usWinDescent = 2210, 514
os2.fsSelection &= ~(1 << 7)
f.save(dst)
g = TTFont(dst)
print('saved', dst, [g['name'].getDebugName(i) for i in (1,2,4,6)], sorted(g.getBestCmap())[-5:], len(g.getBestCmap()))
