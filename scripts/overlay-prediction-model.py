"""Deterministic latency model, not a headset measurement.

python scripts/overlay-prediction-model.py <output.json>
Assumes a 3 ms compositor latch lead, 11 ms display delay, exact constant-velocity
pose extrapolation, 6 ms wide jobs and 60 ms cube jobs. Reports display-space
position error with the old poll loop and the new bounded lifetime prediction.
The baseline uses 11.4 ms turns (including assumed wake overshoot). This model
keeps refreshing during stops; it does not model production motion gating or
GPU contention. These results are not a motion-to-photon measurement.
The production prediction equation is in overlay/worlds/pacing.rs.
"""
import json
import math
import statistics
import sys


def trajectory(t, case):
    if case.startswith("linear") or case == "walking":
        speed = {"linear005": .05, "linear02": .2, "linear05": .5, "walking": .5}[case]
        return (speed*t, 0), (speed, 0)
    if case == "neck-turn":
        yaw = math.radians(35)*math.sin(t*math.tau/4)
        omega = math.radians(35)*math.tau/4*math.cos(t*math.tau/4)
        return (.1*math.sin(yaw)-.032*math.cos(yaw), .1*math.cos(yaw)+.032*math.sin(yaw)), (omega*(.1*math.cos(yaw)+.032*math.sin(yaw)), omega*(-.1*math.sin(yaw)+.032*math.cos(yaw)))
    if t < 2:
        return (.2*t, 0), (.2, 0)
    if t < 3:
        return (.4, 0), (0, 0)
    if t < 5:
        return (.4-.2*(t-3), 0), (-.2, 0)
    return (0, 0), (0, 0)


def simulate(case, hz, rate, beta, baseline_turn=.0114):
    frame = 1/hz
    baseline = beta is None
    next_job = 0
    pending = None
    queued = []
    visible = None
    old_ready = .012
    last_vel = None
    last_sample = 0
    steady_since = 0
    next_display = frame + .011
    errors = []
    stop_errors = []
    cube_centre = 0
    jobs = 0
    next_poll = 0
    for tick in range(60001):
        t = tick/10000
        turn = not baseline or t+1e-9 >= next_poll
        if baseline and turn:
            next_poll += baseline_turn
        if turn and pending and t+1e-9 >= pending[0]:
            ready, predicted, asked = pending
            old_ready = .8*old_ready + .2*(t-asked)
            queued.append((t+.003, predicted))
            pending = None
        if turn and pending is None and t+1e-9 >= next_job:
            p, v = trajectory(t, case)
            cube = case == "walking" and abs(p[0]-cube_centre) >= .45
            cost = .060 if cube else .006
            if cube:
                cube_centre = p[0]
            if last_vel is None or math.dist(v,last_vel)/max(.001,t-last_sample) >= 1:
                steady_since = t
            last_vel = v
            last_sample = t
            until_vsync = frame-t%frame
            if baseline:
                h = min(.1, old_ready+until_vsync+.011)
            else:
                h = until_vsync + max(0, math.ceil((cost+.003-until_vsync)/frame))*frame+.011
                if t-steady_since >= .1 and math.hypot(*v) > .02:
                    h += min(beta/rate, .012)
                h = min(.1,h)
            predicted = tuple(x+d*h for x,d in zip(p,v))
            pending = (t+cost,predicted,t)
            jobs += 1
            next_job = t+1/rate if baseline or t-next_job >= .5/rate else next_job+1/rate
        while t+1e-9 >= next_display:
            vsync = next_display-.011
            while queued and queued[0][0] <= vsync+1e-9:
                _,visible = queued.pop(0)
            if visible is not None:
                actual,_ = trajectory(next_display,case)
                error = math.dist(actual,visible)*1000
                errors.append(error)
                if (2 <= next_display <= 2.2) or (3 <= next_display <= 3.2) or (5 <= next_display <= 5.2):
                    stop_errors.append(error)
            next_display += frame
    percentile = lambda v,q: sorted(v)[int((len(v)-1)*q)]
    return {"case":case,"hz":hz,"rate":rate,"beta":beta,"baseline_turn_ms":baseline_turn*1000,"jobs":jobs,
            "mean_position_error_mm":statistics.mean(errors),"p95_position_error_mm":percentile(errors,.95),
            "stop_window_p95_mm":percentile(stop_errors,.95)}


if __name__ == '__main__':
    rows=[simulate(case,hz,rate,beta) for hz in [72,80,90,120,144]
          for rate in [20,30,45] for case in ["linear005","linear02","linear05","neck-turn","stop-reverse","walking"]
          for beta in [None,0,.25,.5]]
    with open(sys.argv[1],"w") as f:
        json.dump({"assumptions":__doc__,"rows":rows},f,indent=2)
    for row in rows:
        if row["hz"]==90 and row["rate"]==30:
            print(json.dumps(row))
