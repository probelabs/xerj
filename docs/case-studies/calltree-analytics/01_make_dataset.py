# Realistic wifi support conversations. Structured fields + free-text body.
# Deliberately mixes wifi and non-wifi (billing, TV) so retrieval has to discriminate.
import json, random
random.seed(7)
WIFI = [
 ("2.4GHz","range","Wifi drops in the back bedroom, far from the router. Fine in the living room.","resolved"),
 ("2.4GHz","congestion","Slow wifi every evening around 7pm, neighbors all online, 2.4 band crowded.","resolved"),
 ("2.4GHz","interference","Wifi cuts out when the microwave runs. Only the older laptop is affected.","resolved"),
 ("2.4GHz","channel-overlap","Constant lag, wifi analyzer shows 6 networks on channel 6 overlapping.","workaround"),
 ("2.4GHz","legacy-device","Old smart plug won't connect, only supports 2.4GHz, keeps dropping.","resolved"),
 ("5GHz","range","5GHz signal weak two rooms away, drops to 2.4 automatically and slows down.","resolved"),
 ("5GHz","dfs-radar","Wifi disconnects for a minute at random, near an airport, DFS channel radar events.","escalated"),
 ("5GHz","band-steering","Phone keeps switching between 2.4 and 5, drops the video call each time.","workaround"),
 ("5GHz","slow-speed","Paying for gigabit but only getting 300Mbps over 5GHz wifi on the laptop.","resolved"),
 ("5GHz","disconnects","5GHz keeps dropping every few minutes on the new mesh node upstairs.","escalated"),
 ("5GHz","channel-width","Speeds capped, 5GHz stuck at 20MHz channel width instead of 80.","resolved"),
 ("2.4GHz","disconnects","Cameras on 2.4GHz drop offline overnight then reconnect in the morning.","open"),
 ("5GHz","congestion","Apartment building, 5GHz also crowded now, streaming buffers at night.","workaround"),
 ("2.4GHz","slow-speed","2.4GHz only reaching 40Mbps even right next to the router.","resolved"),
 ("5GHz","range","New house, 5GHz doesn't reach the garage office at all.","escalated"),
]
OTHER = [
 ("n/a","billing","Charged twice this month, want a refund on the duplicate charge.","resolved"),
 ("n/a","billing","Bill went up after promo ended, asking for a better rate.","open"),
 ("n/a","tv","Cable box keeps rebooting during live TV, picture freezes.","escalated"),
 ("n/a","outage","Whole street internet is down since the storm last night.","resolved"),
 ("n/a","email","Can't log into my ISP email account, password reset not arriving.","resolved"),
 ("n/a","install","Rescheduling my install appointment to next week.","resolved"),
]
def expand(base, n):
    out=[]
    for i in range(n):
        band,issue,body,res = random.choice(base)
        out.append(dict(band=band,issue=issue,body=body,resolution=res,
                        device=random.choice(["laptop","phone","smart-tv","iot","tablet","desktop"]),
                        csat=random.choice([1,2,3,4,5,4,5,5,3]),
                        handle_time_s=random.randint(120,1800),
                        agent=random.choice(["A. Rivera","J. Chen","M. Okafor","S. Patel"]),
                        channel=random.choice(["voice","chat","email"]),
                        day=random.randint(1,28)))
    return out
docs = expand(WIFI, 90) + expand(OTHER, 40)
random.shuffle(docs)
# canonical KB articles (the "canonical records" to cluster against)
KB = [
 dict(kb_id="KB-2G-RANGE", title="Improving 2.4GHz range and coverage", body="2.4GHz travels farther through walls but is slower and more congested; place the router centrally, reduce interference from microwaves and cordless phones, choose channel 1/6/11."),
 dict(kb_id="KB-5G-DFS", title="5GHz DFS radar disconnects", body="5GHz DFS channels must yield to radar; near airports/weather stations this causes periodic disconnects. Lock the router to a non-DFS 5GHz channel (36-48)."),
 dict(kb_id="KB-BANDSTEER", title="Band steering and roaming drops", body="Band steering moves a device between 2.4 and 5GHz; on calls this can drop the connection. Split the SSIDs or disable band steering for sensitive devices."),
 dict(kb_id="KB-CONGEST", title="Evening wifi congestion", body="Peak-hour slowdowns come from channel congestion in dense areas; use a wifi analyzer, move to a clear channel, prefer 5GHz which has more non-overlapping channels."),
 dict(kb_id="KB-WIDTH", title="Channel width and throughput", body="5GHz throughput depends on channel width (20/40/80/160MHz); if stuck at 20MHz speeds are capped. Enable 80MHz where the spectrum is clean."),
]
json.dump(dict(docs=docs, kb=KB), open("/tmp/claude-1001/-home-claude-ai-xerj/09b4eaff-87bf-4097-b72f-3907d1ef6cd4/scratchpad/rob/dataset.json","w"))
print(f"generated {len(docs)} conversations ({sum(1 for d in docs if d['band']!='n/a')} wifi), {len(KB)} KB articles")
