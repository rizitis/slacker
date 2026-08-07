#!/usr/bin/env python3
# slacker — labyrinth emblem generator.  Ariadne's thread through the dependency maze.
# A unicursal labyrinth: one path, no dead ends.  In the Cretan tradition (Knossos).
import sys, os

K = 5                                    # circuits (try 4..7)
SP = 2

def build(K):
    R = 2*K + 1; cx = cy = R + 1; S = 2*R + 3
    g = [['.']*S for _ in range(S)]
    for rad in range(1, R+1, 2):
        for x in range(cx-rad, cx+rad+1): g[cy-rad][x]='#'; g[cy+rad][x]='#'
        for y in range(cy-rad, cy+rad+1): g[y][cx-rad]='#'; g[y][cx+rad]='#'
    walls = list(range(1, R+1, 2))
    for i, rad in enumerate(walls):
        (g[cy+rad].__setitem__(cx,'.') if i%2==0 else g[cy-rad].__setitem__(cx,'.'))
    g[cy+R][cx]='.'; g[cy+R+1][cx]='.'                 # entrance mouth
    return g, cx, cy, R

def trace(g, cx, cy, R):
    S=len(g); start=(cx,cy+R+1); cur=start; prev=None; path=[cur]; seen={cur}
    while cur!=(cx,cy):
        nb=[(cur[0]+dx,cur[1]+dy) for dx,dy in((0,-1),(0,1),(-1,0),(1,0))
            if 0<=cur[0]+dx<S and 0<=cur[1]+dy<S and g[cur[1]+dy][cur[0]+dx]=='.']
        nb=[p for p in nb if p!=prev and p not in seen]
        if not nb: break
        prev,cur=cur,nb[0]; path.append(cur); seen.add(cur)
    return path

SHARP={(0,0,1,1):'─',(1,1,0,0):'│',(1,0,1,0):'└',(1,0,0,1):'┘',(0,1,1,0):'┌',
 (0,1,0,1):'┐',(1,1,1,0):'├',(1,1,0,1):'┤',(1,0,1,1):'┴',(0,1,1,1):'┬',
 (1,1,1,1):'┼',(1,0,0,0):'│',(0,1,0,0):'│',(0,0,1,0):'─',(0,0,0,1):'─',(0,0,0,0):' '}
RND={'┌':'╭','┐':'╮','└':'╰','┘':'╯'}
HALF={(1,0,0,0):'╵',(0,1,0,0):'╷',(0,0,1,0):'╶',(0,0,0,1):'╴'}

def glyph(cells,x,y,pretty):
    N=(x,y-1)in cells;Sa=(x,y+1)in cells;E=(x+1,y)in cells;W=(x-1,y)in cells
    k=(int(N),int(Sa),int(E),int(W))
    ch=HALF.get(k,SHARP[k]) if pretty else SHARP[k]
    return RND.get(ch,ch) if pretty else ch

WORD=[r"    _         _           ",
      r" __| |__ _ __| |_____ _ _ ",
      r"(_-< / _` / _| / / -_) '_|",
      r"/__/_\__,_\__|_\_\___|_|  "]

def render(K, pretty=False, color=False, mark='♦' if True else '◆'):
    g,cx,cy,R=build(K); th=set(trace(g,cx,cy,R))
    S=len(g); wall={(x,y) for y in range(S) for x in range(S) if g[y][x]=='#'}
    xs=[x for x,y in wall|th]; ys=[y for x,y in wall|th]
    x0,x1,y0,y1=min(xs),max(xs),min(ys),max(ys); w=x1-x0+1
    if color:
        if os.environ.get("TERM","").endswith(("-256color","kitty","alacritty")) or pretty:
            DIM="\033[38;5;238m"; THR="\033[1;38;5;208m"; HRT="\033[1;38;5;220m"
        else:                                  # raw VGA console = 16 colors
            DIM="\033[90m"; THR="\033[1;33m"; HRT="\033[1;31m"
        RST="\033[0m"
    out=[]
    for y in range(y0,y1+1):
        row=''
        for x in range(x0,x1+1):
            if (x,y)==(cx,cy):
                row+= (HRT+mark+RST) if color else mark
            elif (x,y) in th:
                ch=glyph(th,x,y,pretty); row+= (THR+ch+RST) if color else ch
            elif (x,y) in wall:
                ch=glyph(wall,x,y,pretty); row+= (DIM+ch+RST) if color else ch
            else: row+=' '
        out.append(row)
    pad=lambda s:' '*max(0,(w-len(s))//2)+s
    body='\n'.join(out)+'\n\n'+'\n'.join(pad(l) for l in WORD)
    tag="· one thread, no dead ends ·"
    body+='\n'+pad(tag)
    return body

if __name__=='__main__':
    pretty='--pretty' in sys.argv; color='--color' in sys.argv
    print(render(K, pretty=pretty, color=color))
