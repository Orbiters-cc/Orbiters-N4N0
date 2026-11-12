"""OpenVR pixel rays and the installed Quest Pro controller's documented aim component."""
import math

def multiply(a,b):
    w,x,y,z=a;v,i,j,k=b
    return [w*v-x*i-y*j-z*k,w*i+x*v+y*k-z*j,w*j-x*k+y*v+z*i,w*k+x*j-y*i+z*v]

def rotate(q,p):
    return multiply(multiply(q,[0]+list(p)),[q[0],-q[1],-q[2],-q[3]])[1:]

def pointing_pose(view,pixel,size,component):
    head=view['head'];eye=view['eyes'][0];eye_matrix=eye['eye_to_head']
    l,r,t,b=eye['projection'];u,v=[pixel[i]/size[i] for i in range(2)]
    ray=[l+(r-l)*u,b-(b-t)*v,-1]
    ray_head=[sum(eye_matrix[i*4+j]*ray[j] for j in range(3)) for i in range(3)]
    ray_world=[sum(head[i*4+j]*ray_head[j] for j in range(3)) for i in range(3)]
    n=math.sqrt(sum(x*x for x in ray_world));d=[x/n for x in ray_world]
    q=[1-d[2],d[1],-d[0],0]
    n=math.sqrt(sum(x*x for x in q))
    if n<1e-8:q=[0,0,1,0]
    else:q=[x/n for x in q]
    # Component rotation is supplied by SteamVR's render-model JSON, degrees XYZ.
    angles=component['rotate_xyz']
    if angles[1:]!=[0,0]:raise ValueError('unexpected controller aim component')
    angle=math.radians(angles[0])/2
    q=multiply(q,[math.cos(angle),-math.sin(angle),0,0])
    eye_position=[head[i*4+3]+sum(head[i*4+j]*eye_matrix[j*4+3] for j in range(3)) for i in range(3)]
    offset=rotate(q,component['origin'])
    return [eye_position[i]+d[i]*.05-offset[i] for i in range(3)],q
