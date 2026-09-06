import sys
from PIL import Image

path=sys.argv[1]; COLS=int(sys.argv[2]); style=sys.argv[3] if len(sys.argv)>3 else 'ascii'
im=Image.open(path).convert('RGB'); W,H=im.size; px=im.load()
def cls(p):
    r,g,b=p
    if max(p)<=60: return None
    if r>150 and g>150 and b<120: return 'Y'
    if r>150 and 60<g<180 and b<100: return 'O'
    return 'G'
hs=[]; vs=[]
for y in range(H):
    x=0
    while x<W:
        c=cls(px[x,y])
        if c:
            x2=x
            while x2<W and cls(px[x2,y])==c: x2+=1
            if x2-x>6: hs.append((y,x,x2-1,c))
            x=x2
        else: x+=1
for x in range(W):
    y=0
    while y<H:
        c=cls(px[x,y])
        if c:
            y2=y
            while y2<H and cls(px[x,y2])==c: y2+=1
            if y2-y>6: vs.append((x,y,y2-1,c))
            y=y2
        else: y+=1
def merge(segs):
    groups={}
    for a,b,c,col in segs: groups.setdefault((b,c,col),[]).append(a)
    out=[]
    for k,ys in groups.items():
        ys.sort(); run=[ys[0]]
        for y in ys[1:]:
            if y-run[-1]<=2: run.append(y)
            else: out.append((sum(run)//len(run),)+k); run=[y]
        out.append((sum(run)//len(run),)+k)
    return sorted(out)
hs=merge(hs); vs=merge(vs)
raw=sorted({h[0] for h in hs})
ylev=[]; run=[raw[0]]
for y in raw[1:]:
    if y-run[-1]<=5: run.append(y)
    else: ylev.append(sum(run)//len(run)); run=[y]
ylev.append(sum(run)//len(run)); ROWS=len(ylev)
def R(y): return min(range(ROWS), key=lambda i: abs(ylev[i]-y))
def C(x): return max(0,min(COLS-1, round(x/(W-1)*(COLS-1))))
g=[[' ']*COLS for _ in range(ROWS)]
CH={'G':('-','|'),'O':('=','H'),'Y':('~','I')} if style=='ascii' else {'G':('─','│'),'O':('━','┃'),'Y':('╌','╎')}
X='+' if style=='ascii' else '┼'
def put(r,c,ch):
    if g[r][c]==' ': g[r][c]=ch
    elif g[r][c]!=ch: g[r][c]=X
for y,x0,x1,col in hs:
    r=R(y)
    for c in range(C(x0),C(x1)+1): put(r,c,CH[col][0])
for x,y0,y1,col in vs:
    c=C(x)
    for r in range(R(y0),R(y1)+1): put(r,c,CH[col][1])

if style!='ascii':
    HCH=set('─━╌'); VCH=set('│┃╎'); JN='┼'
    JMAP={(0,0,1,1):'┌',(0,1,0,1):'┐',(1,0,1,0):'└',(1,1,0,0):'┘',
          (1,0,1,1):'├',(1,1,0,1):'┤',(0,1,1,1):'┬',(1,1,1,0):'┴',(1,1,1,1):'┼'}
    old=[row[:] for row in g]
    for r in range(ROWS):
        for c in range(COLS):
            if old[r][c]!=JN: continue
            n = r>0 and (old[r-1][c] in VCH or old[r-1][c]==JN)
            s_ = r<ROWS-1 and (old[r+1][c] in VCH or old[r+1][c]==JN)
            w = c>0 and (old[r][c-1] in HCH or old[r][c-1]==JN)
            e = c<COLS-1 and (old[r][c+1] in HCH or old[r][c+1]==JN)
            g[r][c]=JMAP.get((int(n),int(w),int(e),int(s_)),JN)
sys.stderr.write("rows=%d ylev=%s\n"%(ROWS,ylev))
print('\n'.join(''.join(r).rstrip() for r in g))
