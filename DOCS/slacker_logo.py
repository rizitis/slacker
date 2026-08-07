#!/usr/bin/env python3
# slacker — Cretan-labyrinth emblem & banner generator.
# Ariadne's thread through the dependency labyrinth.  Unicursal: one path, no dead ends.
# Heart: the labrys (double axe) — etymological root of  labyrinthos .
#
# usage:
#   slacker_logo.py                 emblem, raw-console safe (CP437), mono
#   slacker_logo.py --pretty        rounded corners, for terminal emulators
#   slacker_logo.py --color         ANSI color (auto 16/256), gated on isatty
#   slacker_logo.py --banner        horizontal banner (emblem + wordmark)
#   flags combine, e.g.  --banner --pretty --color
import sys, os

K_EMBLEM, K_BANNER = 5, 3
VERSION = "v0.x.x-beta.3"

WORD = [r"    _         _           ",
        r" __| |__ _ __| |_____ _ _ ",
        r"(_-< / _` / _| / / -_) '_|",
        r"/__/_\__,_\__|_\_\___|_|  "]

def build(K):
    R=2*K+1; cx=cy=R+1; S=2*R+3
    g=[['.']*S for _ in range(S)]
    for rad in range(1,R+1,2):
        for x in range(cx-rad,cx+rad+1): g[cy-rad][x]='#'; g[cy+rad][x]='#'
        for y in range(cy-rad,cy+rad+1): g[y][cx-rad]='#'; g[y][cx+rad]='#'
    for i,rad in enumerate(range(1,R+1,2)):
        (g[cy+rad].__setitem__(cx,'.') if i%2==0 else g[cy-rad].__setitem__(cx,'.'))
    g[cy+R][cx]='.'; g[cy+R+1][cx]='.'
    return g,cx,cy,R

def trace(g,cx,cy,R):
    S=len(g); cur=(cx,cy+R+1); prev=None; path=[cur]; seen={cur}
    while cur!=(cx,cy):
        nb=[(cur[0]+dx,cur[1]+dy) for dx,dy in((0,-1),(0,1),(-1,0),(1,0))
            if 0<=cur[0]+dx<S and 0<=cur[1]+dy<S and g[cur[1]+dy][cur[0]+dx]=='.']
        nb=[p for p in nb if p!=prev and p not in seen]
        if not nb: break
        prev,cur=cur,nb[0]; path.append(cur); seen.add(cur)
    return path

SHARP={(0,0,1,1):'─',(1,1,0,0):'│',(1,0,1,0):'└',(1,0,0,1):'┘',(0,1,1,0):'┌',
 (0,1,0,1):'┐',(1,1,1,0):'├',(1,1,0,1):'┤',(1,0,1,1):'┴',(0,1,1,1):'┬',(1,1,1,1):'┼',
 (1,0,0,0):'│',(0,1,0,0):'│',(0,0,1,0):'─',(0,0,0,1):'─',(0,0,0,0):' '}
RND={'┌':'╭','┐':'╮','└':'╰','┘':'╯'}
HALF={(1,0,0,0):'╵',(0,1,0,0):'╷',(0,0,1,0):'╶',(0,0,0,1):'╴'}
LAB_SAFE  ={(0,-1):'│',(-1,0):'◄',(0,0):'┼',(1,0):'►',(0,1):'│'}
LAB_PRETTY={(0,-1):'┃',(-1,0):'◀',(0,0):'╋',(1,0):'▶',(0,1):'┃'}

def glyph(cells,x,y,pretty):
    k=(int((x,y-1)in cells),int((x,y+1)in cells),int((x+1,y)in cells),int((x-1,y)in cells))
    ch=HALF.get(k,SHARP[k]) if pretty else SHARP[k]
    return RND.get(ch,ch) if pretty else ch

def palette(color,pretty):
    if not color: return (lambda kind,s:s)
    if pretty or os.environ.get("TERM","").endswith(("256color","kitty","alacritty","foot","direct")):
        C={'wall':"\033[38;5;238m",'thread':"\033[1;38;5;208m",'heart':"\033[1;38;5;220m",
           'word':"\033[1;38;5;208m",'dim':"\033[38;5;245m"}
    else:
        C={'wall':"\033[90m",'thread':"\033[1;33m",'heart':"\033[1;31m",
           'word':"\033[1;33m",'dim':"\033[37m"}
    R="\033[0m"
    return (lambda kind,s: C[kind]+s+R)

def emblem_rows(K,pretty,color):
    g,cx,cy,R=build(K); th=set(trace(g,cx,cy,R))
    S=len(g); wall={(x,y) for y in range(S) for x in range(S) if g[y][x]=='#'}
    lab=LAB_PRETTY if pretty else LAB_SAFE; heart={(cx+dx,cy+dy) for dx,dy in lab}
    col=palette(color,pretty)
    xs=[x for x,y in wall|th]; ys=[y for x,y in wall|th]
    x0,x1,y0,y1=min(xs),max(xs),min(ys),max(ys)
    rows=[]
    for y in range(y0,y1+1):
        r=''
        for x in range(x0,x1+1):
            if (x,y) in heart:            r+=col('heart',lab[(x-cx,y-cy)])
            elif abs(x-cx)<=1 and abs(y-cy)<=1: r+=' '
            elif (x,y) in th:             r+=col('thread',glyph(th,x,y,pretty))
            elif (x,y) in wall:           r+=col('wall',glyph(wall,x,y,pretty))
            else:                         r+=' '
        rows.append(r)
    return rows,(x1-x0+1)

def render_emblem(pretty,color):
    rows,w=emblem_rows(K_EMBLEM,pretty,color)
    col=palette(color,pretty)
    pad=lambda s:' '*max(0,(w-len(s))//2)+s
    out='\n'.join(rows)
    out+='\n\n'+'\n'.join(pad(col('word',l)) for l in WORD)
    out+='\n'+pad(col('dim',"· one thread, no dead ends ·"))
    return out

def render_banner(pretty,color):
    rows,ew=emblem_rows(K_BANNER,pretty,color)
    # ljust by VISIBLE width (strip ANSI for measuring)
    import re; vis=lambda s:len(re.sub(r'\033\[[0-9;]*m','',s))
    rows=[r+' '*(ew-vis(r)) for r in rows]
    col=palette(color,pretty)
    right=[col('word',l) for l in WORD]+['',
           col('dim',"one thread, no dead ends"),
           col('dim',"binary packages · "+VERSION)]
    H=len(rows); top=(H-len(right))//2
    rb=['']*top+right; rb+=['']*(H-len(rb))
    return '\n'.join((rows[i]+'   '+rb[i]).rstrip() for i in range(H))

if __name__=='__main__':
    a=sys.argv[1:]
    pretty='--pretty' in a
    color=('--color' in a) and (sys.stdout.isatty() and os.environ.get("NO_COLOR") is None or '--force-color' in a)
    print(render_banner(pretty,color) if '--banner' in a else render_emblem(pretty,color))
