"""The SMP on a real dedicated server: the built game runs `--dedicated` with the mod, and two real clients (Alex and
Steve, `horizon-ctl --offscreen`) join it. Checked from the clients' own chat and from the files the server wrote:

- the economy opens an account for each player (config.json written with its defaults, then economy.json);
- /balance, /pay (the payer's answer and the payee's line), refused payments (too much, to oneself, unknown name);
- money from an operator (made one from the server's console): /eco give with a reason everyone reads; a
  non-operator can't; /treasury, /bank, /jobs, /futures and /eco taxes are gone, and the old Treasury pool is dropped;
- /baltop;
- the commands are in each player's command tree (the chat box's suggestions);
- the market (Steve in survival, items given from the console): /worth, /sell <item>, /sell hand, /buy, with the
  items gone from or in his inventory as his client sees it; a creative player can't sell;
- trades (`/trade`, the server's chest menu clicked from each client): closing the menu, a full inventory, the
  cancel button, dying and leaving each end or refuse a trade with nothing moved (each player's items counted from
  their own client); a change of offer clears the other's confirmation; a confirmed trade swaps the items and money;
- coinflips (turned on in config.json before the first start, the other settings filled in): the stake is held from
  the offer, the winner gets both, legacy limits and cooldowns are ignored, and a stake left waiting when the server stops
  comes back; the money in the game stays the same throughout;
- the lottery: two players' tickets held, one drawn after `lottery_seconds`, the winner announced to everyone and paid
  the pot;
- after the server stops cleanly and starts again, the balances are the same, and audit.log holds every change;
- an operator's /eco give, then the /shop menu: a category, one item bought by a click and a stack by
  a shift-click, a stack sold by clicking it, unlimited "Sell all", the close button;
- after the restart, prices moving: a purchase and a sale with no tax (receipts), an item bought can be sold
  back immediately, one price for diamond, moved down by a sale and up by a purchase as they go, /market's item
  page buying, the 24 h chart given as a map and drawn in the player's hand (market-chart.png), web/market.json, and
  market_prices.json written when the world closes;
- the web pages (web/market.json, web/economy.json) sent gzipped with the token to a stand-in for Kira;
- /invsee: an operator takes a stack from an offline player's saved inventory, logged before the move and summed up in
  staff.log;
- nicknames, for operators only: a player can't pick one; an operator's refused ones (a player's name, too short, a
  bad character), the one given in the other's chat and Tab list, /realname, someone else's taken, the player taking
  it off.

The casino has its own proof (proof_casino.py): `SMP_PROOF=casino` runs it instead of this one.

Everything runs in fresh temporary folders (the server's, each client's home, launcher and XDG folders), no audio.
"""
import gzip
import http.server
import json
import math
import os
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from pathlib import Path

PROPERTIES = {
    "online-mode": "false", "white-list": "false", "enforce-secure-profile": "false", "level-type": "minecraft\\:flat", "spawn-protection": "0", "max-players": "4",
    "view-distance": "4", "simulation-distance": "4", "generate-structures": "false", "enable-command-block": "false",
    "difficulty": "peaceful", "spawn-monsters": "false",
}


class Receiver:
    """Stands in for Kira's POST /smp/ingest/<name>: keeps what the server sends (its path, bearer, unzipped body)."""

    def __init__(self):
        got = self.got = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
                if self.headers.get("Content-Encoding") == "gzip":
                    body = gzip.decompress(body)
                got.append((self.path, self.headers.get("Authorization"), json.loads(body)))
                self.send_response(200)
                self.end_headers()

            def log_message(self, *a):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/smp/ingest"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def scratch_env(root):
    env = dict(os.environ, HORIZON_NO_AUDIO="1", HORIZON_LAUNCHER_NO_SELF_UPDATE="1")
    for key in ("HORIZON_HOME", "HORIZON_LAUNCHER_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME"):
        path = root / key.lower()
        path.mkdir(parents=True, exist_ok=True)
        env[key] = str(path)
    return env


class Server:
    """`horizon-client --dedicated --nogui` in a scratch server folder, its console on stdin."""

    def __init__(self, game, resources, folder, port):
        self.folder, self.lines, self.game, self.resources, self.port = folder, [], game, resources, port
        folder.mkdir(parents=True, exist_ok=True)
        props = dict(PROPERTIES, **{"server-port": str(port)})
        (folder / "server.properties").write_text("".join(f"{k}={v}\n" for k, v in props.items()))
        (folder / "eula.txt").write_text("eula=true\n")
        self.env = scratch_env(folder.parent / "server-env")

    def start(self):
        self.lines = []
        self.process = subprocess.Popen(
            [str(self.game), "--resources", str(self.resources), "--dedicated", "--nogui"], cwd=self.folder,
            env=self.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        threading.Thread(target=self._read, daemon=True).start()
        self.wait_for("Done (", 600)

    def _read(self):
        for line in self.process.stdout:
            self.lines.append(line.rstrip("\n"))

    def wait_for(self, text, timeout=120):
        end = time.time() + timeout
        while time.time() < end:
            if any(text in line for line in self.lines):
                return
            if self.process.poll() is not None:
                break
            time.sleep(0.2)
        raise AssertionError(f"server: no {text!r} in {timeout} s; last lines: {self.lines[-30:]}")

    def console(self, line):
        self.process.stdin.write(line + "\n")
        self.process.stdin.flush()

    def stop(self):
        if self.process.poll() is None:
            self.console("stop")
            try:
                self.process.wait(120)
            except subprocess.TimeoutExpired:
                self.process.kill()
        return self.lines


def new_lines(before, after):
    """The lines `after` adds to `before`: the client keeps only its last 100 chat lines, so the oldest may be gone."""
    for n in range(min(len(before), len(after)), -1, -1):
        if before[len(before) - n:] == after[:n]:
            return after[n:]
    return after


class Client:
    """A real client under horizon-ctl, joined to the server as `name`."""

    def __init__(self, ctl, resources, name, root):
        self.ctl, self.name, self.resources = ctl, name, resources
        self.home = root / name
        self.env = scratch_env(self.home)
        self.socket = str(self.home / "ctl.sock")

    def run(self, *args, check=True):
        done = subprocess.run([str(self.ctl), "--socket", self.socket, *map(str, args)], env=self.env,
                              cwd=self.home, capture_output=True, text=True, timeout=600)
        if check and done.returncode:
            raise AssertionError(f"{self.name}: {args}: {done.stdout[-400:]} {done.stderr[-400:]}")
        try:
            return json.loads(done.stdout.strip().splitlines()[-1])
        except (json.JSONDecodeError, IndexError):
            return done.stdout

    def join(self, port):
        self.run("launch", "--offscreen", "--resources", self.resources)
        self.run("connect", f"127.0.0.1:{port}", self.name)
        self.run("wait-for", "loaded", "--timeout", 300)
        time.sleep(2)

    def say(self, line):
        """A command typed in chat; returns the chat lines it brought."""
        before = self.chat()
        self.run("chat", line)
        # The answer, however busy the machine: up to 10 s for a first line,
        # then a moment for the rest of it.
        for _ in range(40):
            time.sleep(0.25)
            if self.chat() != before:
                break
        time.sleep(1)
        return new_lines(before, self.chat())

    def chat(self):
        """Every chat line so far, oldest first (`messages` answers newest first)."""
        reply = self.run("messages", 500)
        return list(reversed(reply.get("chat", [])))

    def quit(self):
        self.run("quit", check=False)

    def screen(self):
        return self.run("screen")

    def menu(self):
        """The open server menu's items by its own slot (horizon-ctl numbers them from 300)."""
        return {e["slot"] - 300: e for e in self.screen().get("slots", []) if e.get("item") and e["slot"] >= 300}

    def slot_of(self, item):
        """The inventory slot (0..36) holding `item`, as the open screen shows it."""
        return next(e["slot"] for e in self.screen().get("slots", []) if e.get("item") == item and e["slot"] < 36)

    def count(self, item):
        counts = self.run("inventory").get("counts", {})
        return counts.get(item, counts.get(item.removeprefix("minecraft:"), 0))

    def click(self, slot):
        self.run("click-slot", slot)
        time.sleep(1)


CONFIRM, CANCEL, THEIR_FIRST = 300 + 45, 300 + 48, 5


def open_trade(alex, steve):
    assert any("Trade request sent to Steve." in l for l in alex.say("/trade Steve"))
    assert any("Alex wants to trade with you" in l for l in steve.chat()[-3:])
    steve.say("/trade accept")
    time.sleep(1)
    for c, other in ((alex, "Steve"), (steve, "Alex")):
        menu = c.menu()
        assert menu.get(45, {}).get("item") == "minecraft:lime_stained_glass_pane", (c.name, menu)
        title = "".join(s["text"] for s in c.screen().get("title", []))
        assert title == f"Trade with {other}", title


def coinflips(alex, steve):
    """Alex 415, Steve 400. Returns what was seen and whether Alex won."""
    results = {}
    results["invalid"] = alex.say("/coinflip 0")
    assert any("Usage:" in l for l in results["invalid"]), results
    results["offer"] = alex.say("/coinflip 100 Steve")
    assert any("Coinflip of 100 coins offered to Steve" in l for l in results["offer"]), results
    assert any("Balance: 315 coins" in l for l in alex.say("/balance")), "the stake is held"
    assert any("Alex offers you a coinflip of 100 coins" in l for l in steve.chat()[-3:])
    steve.say("/coinflip accept Alex")
    time.sleep(1)
    results["alex_saw"] = alex.chat()[-1:]
    alex_wins = any("you won 200 coins" in l for l in results["alex_saw"])
    assert alex_wins or any("you lost 100 coins" in l for l in results["alex_saw"]), results
    steve_saw = steve.chat()[-2:]
    assert any(("you lost 100 coins" if alex_wins else "you won 200 coins") in l for l in steve_saw), steve_saw
    assert any(f"Balance: {415 + (100 if alex_wins else -100)} coins" in l for l in alex.say("/balance"))
    assert any(f"Balance: {400 + (-100 if alex_wins else 100)} coins" in l for l in steve.say("/balance"))
    results["no_cooldown"] = steve.say("/coinflip 50")
    assert any("Coinflip of 50 coins offered" in l for l in results["no_cooldown"]), results
    return results, alex_wins


def lottery(alex, steve, alex_wins):
    """Alex buys 3 tickets, Steve 2; the round draws 30 s after the first. Returns what was seen, whether Alex won."""
    results = {}
    alex_had = 415 + (100 if alex_wins else -100)
    steve_had = 400 + (-100 if alex_wins else 100) - 50  # his coinflip of 50 is still held
    results["buy"] = alex.say("/lottery buy 3")
    assert any("You bought 3 ticket(s) for 30 coins; the pot is 30 coins" in l for l in results["buy"]), results
    assert any("the pot is 50 coins" in l for l in steve.say("/lottery buy 2"))
    assert any(f"Balance: {alex_had - 30} coins" in l for l in alex.say("/balance")), "the tickets are held"
    results["status"] = steve.say("/lottery")
    assert any("Lottery: pot 50 coins, 2 ticket(s) of yours" in l for l in results["status"]), results
    time.sleep(32)
    results["draw"] = [l for l in alex.chat()[-3:] if l.startswith("Lottery:")]
    assert len(results["draw"]) == 1 and "won 50 coins" in results["draw"][0], results
    assert results["draw"][0] in steve.chat()[-3:], "everyone hears the draw"
    alex_lotto = results["draw"][0].startswith("Lottery: Alex won")
    results["alex_after"] = alex.say("/balance")
    assert any(f"Balance: {alex_had - 30 + (50 if alex_lotto else 0)} coins" in l for l in results["alex_after"]), (
        results, alex_had, alex.chat()[-12:])
    results["steve_after"] = steve.say("/balance")
    assert any(f"Balance: {steve_had - 20 + (0 if alex_lotto else 50)} coins" in l for l in results["steve_after"]), (
        results, steve_had, steve.chat()[-12:])
    return results, alex_lotto


def where(c):
    """The client's dimension, position and health."""
    s = c.run("state")
    return s.get("dimension"), s["player"]["position"], s["player"]["health"]


def near(a, b):
    return sum((x - y) ** 2 for x, y in zip(a, b)) < 1.5 ** 2


def places(server, alex, steve):
    """Spawn, homes, warps, /tpa and /rules; teleports across levels, called off by a move or a hit."""
    r = {}
    r["setspawn"] = alex.say("/setspawn")
    assert any("The spawn is here now." in l for l in r["setspawn"]), r
    r["sethome"] = steve.say("/sethome base")
    assert any('Home "base" set here.' in l for l in r["sethome"]), r
    _, home, _ = where(steve)
    r["limit"] = steve.say("/sethome two")
    assert any("You have 1 of 1 homes" in l for l in r["limit"]), r
    # To the Nether's bedrock roof, then home, across levels.
    server.console("execute in minecraft:the_nether run tp Steve 0.5 128 0.5")
    time.sleep(5)
    assert where(steve)[0] == "minecraft:the_nether", where(steve)
    r["home"] = steve.say("/home base")
    assert any("Teleporting in 2 s: don't move." in l for l in r["home"]), r
    time.sleep(5)
    dim, at, _ = where(steve)
    r["home_reached"] = [dim, at]
    assert dim == "minecraft:overworld" and near(at, home), (r, home)
    assert any('Teleported to home "base".' in l for l in steve.chat()[-4:]), steve.chat()[-4:]
    # A move or a hit during the warmup calls it off.
    steve.say("/spawn")
    steve.run("move", "forward", 1)
    time.sleep(2)
    r["moved"] = steve.chat()[-3:]
    assert any("Teleport called off: you moved." in l for l in r["moved"]), r
    steve.say("/spawn")
    server.console("damage Steve 2")
    time.sleep(3)
    r["hurt"] = steve.chat()[-3:]
    assert any("Teleport called off: you were hurt." in l for l in r["hurt"]), r
    # A request: Alex goes to Steve once Steve accepts.
    alex.say("/tpa Steve")
    assert any("Alex asks to teleport to you" in l for l in steve.chat()[-3:]), steve.chat()[-3:]
    steve.say("/tpaccept")
    time.sleep(4)
    r["tpa"] = [where(alex)[1], where(steve)[1]]
    assert near(*r["tpa"]), r
    alex.say("/setwarp shop")
    r["warps"] = steve.say("/warps")
    assert any("Warps: shop" in l for l in r["warps"]), r
    r["warp_refused"] = steve.say("/setwarp mine")
    assert not any("set here" in l for l in r["warp_refused"]), r
    r["rules"] = steve.say("/rules")
    assert any("1. Be respectful to everyone." in l for l in r["rules"]), r
    return r


def claims(server, alex, steve):
    """Alex claims a chunk: Steve can't break, use or place there until trusted."""
    r = {}
    _, (ax, ay, az), _ = where(alex)
    cx, cz, y = math.floor(ax) >> 4, math.floor(az) >> 4, math.floor(ay)
    r["claim"] = alex.say("/claim")
    assert any("Claimed this chunk" in l for l in r["claim"]), r
    lever, dirt, free = (cx * 16 + 8, y, cz * 16 + 8), (cx * 16 + 10, y, cz * 16 + 8), (cx * 16 + 8, y, cz * 16 + 10)
    server.console("setblock %d %d %d minecraft:lever[face=floor]" % lever)
    server.console("setblock %d %d %d minecraft:dirt" % dirt)
    server.console(f"tp Steve {lever[0] + 0.5} {y} {lever[2] + 3.5}")
    server.console("give Steve minecraft:oak_planks 4")
    time.sleep(3)
    block = lambda pos: json.dumps(steve.run("block", *pos))
    steve.run("interact", *lever)
    time.sleep(0.5)
    # The action bar fades after 3 s: read right after the click (a busy machine may take that long to break).
    said = json.dumps(steve.run("messages", 5))
    steve.run("break", *dirt, "--timeout", 4, check=False)
    steve.run("place", *free, "--item", "minecraft:oak_planks", check=False)
    time.sleep(1.5)
    r["refused"] = [block(lever), block(dirt), block(free), said + json.dumps(steve.run("messages", 5))]
    # Not broken (grass may spread onto the dirt meanwhile).
    assert "powered=true" not in r["refused"][0] and "minecraft:air" not in r["refused"][1], r
    assert "oak_planks" not in r["refused"][2], r
    assert "This land is claimed by Alex." in r["refused"][3], r
    r["trust"] = alex.say("/trust Steve")
    assert any("Steve may now build" in l for l in r["trust"]), r
    steve.run("interact", *lever)
    steve.run("break", *dirt, "--timeout", 10, check=False)
    time.sleep(1.5)
    r["trusted"] = [block(lever), block(dirt)]
    assert "powered=true" in r["trusted"][0] and '"block": "minecraft:air"' in r["trusted"][1], r
    r["claims"] = alex.say("/claims")
    assert any("Your chunks (1 of 4)" in l for l in r["claims"]) and any("Trusted: Steve" in l for l in r["claims"]), r
    return r


def world_guard(server, alex, steve):
    """Nothing reaches Alex's chunk from outside: water, fire, a piston, TNT, Steve's bucket and fists."""
    r = {}
    _, (ax, ay, az), _ = where(alex)
    x0, z0, y = (math.floor(ax) >> 4) * 16, (math.floor(az) >> 4) * 16, math.floor(ay)
    r["untrust"] = alex.say("/untrust Steve")
    block = lambda x, yy, z: json.dumps(steve.run("block", x, yy, z))
    for line in ("weather clear", f"tp Alex {x0 + 8.5} {y} {z0 - 12.5}", f"tp Steve {x0 + 4.5} {y} {z0 - 1.5}",
                 # Water two blocks east of the chunk; fire on netherrack against planks on both sides of its
                 # west edge (more planks outside, so some burn soon).
                 f"setblock {x0 + 17} {y} {z0 + 8} minecraft:water",
                 f"setblock {x0 - 1} {y - 1} {z0 + 8} minecraft:netherrack",
                 f"setblock {x0} {y} {z0 + 8} minecraft:oak_planks",
                 f"setblock {x0 - 2} {y} {z0 + 8} minecraft:oak_planks",
                 f"setblock {x0 - 1} {y} {z0 + 7} minecraft:oak_planks",
                 f"setblock {x0 - 1} {y} {z0 + 9} minecraft:oak_planks",
                 f"setblock {x0 - 1} {y + 1} {z0 + 8} minecraft:oak_planks",
                 f"setblock {x0 - 1} {y} {z0 + 8} minecraft:fire",
                 # North of the chunk: a piston pushing dirt into it, another pushing away from it.
                 f"setblock {x0 + 8} {y} {z0 - 2} minecraft:piston[facing=south]",
                 f"setblock {x0 + 8} {y} {z0 - 1} minecraft:dirt",
                 f"setblock {x0 + 12} {y} {z0 - 2} minecraft:piston[facing=north]",
                 f"setblock {x0 + 12} {y} {z0 - 3} minecraft:dirt",
                 f"setblock {x0 + 8} {y} {z0 - 3} minecraft:redstone_block",
                 f"setblock {x0 + 12} {y} {z0 - 1} minecraft:redstone_block",
                 "give Steve minecraft:water_bucket",
                 f"summon minecraft:cow {x0 + 5.5} {y} {z0 + 0.5} {{NoAI:1b}}"):
        server.console(line)
    time.sleep(4)
    r["water"] = [block(x0 + 16, y, z0 + 8), block(x0 + 15, y, z0 + 8)]
    assert "water" in r["water"][0] and "water" not in r["water"][1], r
    r["pistons"] = [block(x0 + 8, y, z0 - 2), block(x0 + 8, y, z0 - 1), block(x0 + 12, y, z0 - 2)]
    assert "extended=false" in r["pistons"][0] and ("dirt" in r["pistons"][1] or "grass_block" in r["pistons"][1]), r
    assert "extended=true" in r["pistons"][2], r
    # Steve's bucket on the chunk's ground: no water; then on the ground outside it.
    steve.run("look-at-block", x0 + 3, y - 1, z0 + 1, "up")
    steve.run("use-item")
    time.sleep(1.5)
    r["bucket"] = [block(x0 + 3, y, z0 + 1), json.dumps(steve.run("messages", 5))]
    assert "water" not in r["bucket"][0] and "This land is claimed by Alex." in r["bucket"][1], r
    # Steve's fist on Alex's cow: nothing, until Alex trusts him.
    cow = steve.run("entities", 8, "cow")["entities"][0]
    steve.run("attack", cow["id"])
    time.sleep(1.5)
    r["cow"] = [steve.run("entities", 8, "cow")["entities"][0]["health"]]
    r["trust"] = alex.say("/trust Steve")
    steve.run("attack", cow["id"])
    time.sleep(1.5)
    r["cow"].append(steve.run("entities", 8, "cow")["entities"][0]["health"])
    assert r["cow"][0] == 10 and r["cow"][1] < 10, r
    # Fire: the planks outside burn, those inside never.
    for _ in range(90):
        outside = [block(x0 - 2, y, z0 + 8), block(x0 - 1, y, z0 + 7), block(x0 - 1, y, z0 + 9),
                   block(x0 - 1, y + 1, z0 + 8)]
        if sum("oak_planks" not in b for b in outside) >= 2:
            break
        time.sleep(1)
    r["fire"] = outside + [block(x0, y, z0 + 8)]
    assert sum("oak_planks" not in b for b in outside) >= 1 and "oak_planks" in r["fire"][-1], r
    # TNT two blocks south of the chunk: a crater outside, not one block of the chunk broken.
    inside = lambda: json.dumps(steve.run("blocks", x0 + 4, y - 4, z0 + 12, x0 + 12, y + 2, z0 + 15)["counts"])
    r["before_tnt"] = inside()
    server.console(f"summon minecraft:tnt {x0 + 8.5} {y} {z0 + 17.5} {{fuse:1}}")
    for _ in range(20):
        time.sleep(1)
        if "grass" not in block(x0 + 8, y - 1, z0 + 17):
            break
    time.sleep(1)
    r["after_tnt"] = [inside(), block(x0 + 8, y - 1, z0 + 17)]
    assert r["after_tnt"][0] == r["before_tnt"] and "grass" not in r["after_tnt"][1], r
    return r


def board(alex, steve, out):
    """The scoreboard sidebar (the player's own block, the richest, the address) and the Tab list; /sidebar off
    hides it, on shows it again."""
    r = {}
    side = lambda c: c.run("status").get("sidebar")
    for _ in range(20):
        r["sidebar"] = side(steve)
        if r["sidebar"]:
            break
        time.sleep(0.5)
    assert r["sidebar"]["objective"] == "horizon:native_sidebar" and r["sidebar"]["title"] == "Horizon SMP", r
    # Steve's name, balance, playtime and the players on; a gap, "Richest", Alex and Steve (the two accounts); a gap,
    # the address.
    assert len(r["sidebar"]["scores"]) == 10, r
    # Holders starting with '#' would be hidden from the sidebar (only the title would show).
    assert not any(o.startswith("#") for o in r["sidebar"]["scores"]), r
    steve.run("key", "tab", "down")
    time.sleep(1)
    steve.run("screenshot", str(Path(out).resolve() / "sidebar-and-tab.png"))
    steve.run("key", "tab", "up")
    r["off"] = steve.say("/sidebar off")
    assert any("The sidebar is off" in l for l in r["off"]), r
    time.sleep(1)
    assert not side(steve), side(steve)
    assert side(alex), "only Steve's is gone"
    r["on"] = steve.say("/sidebar")
    assert any("The sidebar is on" in l for l in r["on"]), r
    for _ in range(10):
        if side(steve):
            break
        time.sleep(0.5)
    assert side(steve), "back within a second or two"
    steve.say("/sidebar off")
    return r


def polish(server, alex, steve, out):
    """/back after a death, /playtime, /claim show."""
    r = {}
    _, died_at, _ = where(steve)
    server.console("kill Steve")
    time.sleep(2.5)
    steve.run("respawn")
    time.sleep(2)
    r["died_at"], r["respawned_at"] = died_at, where(steve)[1]
    r["back"] = steve.say("/back")
    assert any("Teleporting in" in l for l in r["back"]), r
    time.sleep(4)
    r["back_reached"] = where(steve)[1]
    assert near(r["back_reached"], died_at), r
    assert any("where you died" in l for l in steve.chat()[-4:]), steve.chat()[-4:]
    r["playtime"] = steve.say("/playtime")
    assert any("You have played here for" in l for l in r["playtime"]), r
    r["playtime_alex"] = steve.say("/playtime Alex")
    assert any("Alex has played here for" in l for l in r["playtime_alex"]), r
    r["show"] = alex.say("/claim show")
    assert any("This chunk's border, for 10 s" in l for l in r["show"]), r
    time.sleep(2)
    alex.run("screenshot", str(Path(out).resolve() / "claim-show.png"))
    return r


def presence(server, alex, steve):
    """/afk keeps Steve out of the sleep count (Alex alone in bed passes the night); moving brings him back; too many
    commands at once wait."""
    r = {}
    r["afk"] = steve.say("/afk")
    assert any("You are AFK" in l for l in r["afk"]), r
    _, (ax, ay, az), _ = where(alex)
    x, y, z = math.floor(ax) + 2, math.floor(ay), math.floor(az)
    for line in (f"setblock {x} {y} {z} minecraft:red_bed[part=foot,facing=south]",
                 f"setblock {x} {y} {z + 1} minecraft:red_bed[part=head,facing=south]", "time set 18000"):
        server.console(line)
    time.sleep(2)
    alex.run("interact", x, y, z)
    # Five seconds asleep, then the morning; Steve, AFK and awake, doesn't hold it back.
    for _ in range(40):
        r["time_of_day"] = alex.run("status")["day_time"] % 24000
        if r["time_of_day"] < 12000:
            break
        time.sleep(0.5)
    assert r["time_of_day"] < 12000, r
    steve.run("move", "forward", 1)
    time.sleep(2.5)
    assert any("You are back." in l for l in steve.chat()[-4:]), steve.chat()[-4:]
    for _ in range(9):
        steve.run("chat", "/bal")
    time.sleep(2)
    r["spam"] = steve.chat()[-12:]
    assert any("Slow down: at most 8 commands every 5 s." in l for l in r["spam"]), r
    return r


def nicks(server, alex, steve, out):
    """Nicknames are for operators: Steve can't pick one; Alex (an operator) gives him one, refused ones first, then
    ~Rocky in Alex's chat and the Tab list, /realname, /pay by nickname, and Steve takes it off himself. The one an
    older version let through (a player's own pick) is gone."""
    r = {}
    r["purged"] = [l for l in server.lines if "taken off" in l]
    assert any("nickname N0tch of" in l and "for operators only" in l for l in r["purged"]), r
    time.sleep(6)  # the spam guard's window from the step before
    for line in ("/nick", "/nick Rocky"):
        r[line] = steve.say(line)
        assert any("Only operators set nicknames on this server." in l for l in r[line]), r
    for line, error in (("/nick Steve Alex", "Alex looks like the name of another player."),
                        ("/nick Steve A1ex_", "A1ex_ looks like the name of another player."),
                        ("/nick Steve N0tch", "N0tch reads as \"notch\": that word is kept for the server."),
                        ("/nick Steve ab", "A nickname is 3 to 16 letters, digits or _.")):
        r[line] = alex.say(line)
        assert any(error in l for l in r[line]), r
    time.sleep(6)
    alex.run("chat", "/nick Steve Rocky")
    time.sleep(3)  # made at the next second's tick
    r["set"] = steve.chat()[-3:]
    assert any("Steve now goes by ~Rocky." in l for l in r["set"]), r
    steve.run("chat", "hello from rocky")
    time.sleep(2)
    r["chat"] = alex.chat()[-3:]
    assert any("~Rocky" in l and "hello from rocky" in l and "Steve" not in l for l in r["chat"]), r
    if os.environ.get("SMP_PROOF_MSG_NICK"):
        # An engine whose player arguments take nicknames (horizon fix/commands-take-nicknames).
        alex.run("chat", "/msg Rocky psst")
        time.sleep(2)
        r["msg"] = steve.chat()[-3:]
        assert any("whispers to you: psst" in l for l in r["msg"]), r
    alex.run("key", "tab", "down")
    time.sleep(1)
    alex.run("screenshot", str(Path(out).resolve() / "nick-tab.png"))
    alex.run("key", "tab", "up")
    r["realname"] = alex.say("/realname ~rocky")
    assert any("~Rocky is Steve." in l for l in r["realname"]), r
    r["taken"] = alex.say("/nick R0cky")
    assert any("R0cky looks like the name of another player." in l for l in r["taken"]), r
    # A nickname where a command names a player; paid back so the sums stay.
    r["pay"] = alex.say("/pay Rocky 1")
    assert any("You paid 1 coins to Steve." in l for l in r["pay"]), r
    steve.say("/pay Alex 1")
    r["another"] = steve.say("/nick Pebble")
    assert any("Only operators set nicknames on this server." in l for l in r["another"]), r
    time.sleep(6)
    steve.run("chat", "/nick off")
    time.sleep(3)
    r["off"] = steve.chat()[-3:]
    assert any("Steve goes by their name again." in l for l in r["off"]), r
    steve.run("chat", "back to steve")
    time.sleep(2)
    assert any("Steve" in l and "back to steve" in l for l in alex.chat()[-3:]), alex.chat()[-3:]
    return r


def shop(server, alex):
    """Alex alone, op, in survival: given money (/eco give), then the /shop menu: a category, buying one and a stack,
    selling a stack of hers, "Sell all" without daily limits, closing. Returns the coins it added, and the lines."""
    results = {}
    # Only what the step gives her: "Sell all" would sell the rest too.
    server.console("clear Alex")
    time.sleep(1)
    results["money"] = alex.say("/eco give Alex 2010 proof: shop funds")
    assert any("Alex gave 2,010 coins to Alex: proof: shop funds." in l for l in results["money"]), results
    assert alex.count("minecraft:iron_ingot") == 0
    alex.say("/shop")
    menu = alex.menu()
    assert menu[10]["item"] == "minecraft:wheat" and menu[48]["item"] == "minecraft:gold_ingot", menu
    alex.click(300 + 11)  # Mining
    menu = alex.menu()
    assert [menu[i]["item"] for i in range(3)] == ["minecraft:coal", "minecraft:copper_ingot",
                                                   "minecraft:iron_ingot"], menu
    alex.click(300 + 2)  # one iron ingot
    assert alex.count("minecraft:iron_ingot") == 1, alex.run("inventory")
    results["bought"] = alex.chat()[-3:]
    # One price, 15.
    assert any("Bought 1 iron ingot at 15 coins each: 15 coins." in l for l in results["bought"]), results
    alex.click(300 + 45)  # back
    alex.click(300 + 14)  # Building
    alex.run("click-slot", 300 + 0, "shift")  # a stack of cobblestone
    time.sleep(1)
    assert alex.count("minecraft:cobblestone") == 64, alex.run("inventory")
    # Along the price as the stack moves it: 64 at 1 coin come to 64.05, 65 whole coins.
    results["stack"] = alex.chat()[-4:]  # an advancement may come after
    assert any("Bought 64 cobblestone at 1.02 coins each: 65 coins." in l for l in results["stack"]), results
    assert not any("Tax" in l for l in results["stack"]), results
    # A purchased ingot can be resold immediately.
    alex.click(alex.slot_of("minecraft:iron_ingot"))
    results["round_trip"] = alex.chat()[-4:]
    assert any("Sold 1 iron ingot" in l for l in results["round_trip"]), results
    assert alex.count("minecraft:iron_ingot") == 0
    server.console("give Alex minecraft:iron_ingot 1")
    time.sleep(2)
    alex.click(alex.slot_of("minecraft:iron_ingot"))
    results["sold"] = alex.chat()[-4:]
    # Sold at the price she bought at: one price.
    assert any("Sold 1 iron ingot at 15 coins each: 15 coins." in l for l in results["sold"]), results
    assert alex.count("minecraft:iron_ingot") == 0
    # All thirty diamonds sell, beyond the old daily and minute caps.
    server.console("give Alex minecraft:diamond 30")
    time.sleep(2)
    alex.click(300 + 49)
    results["sell_all"] = alex.chat()[-8:]
    assert any("Sold 30 diamond at 186 coins each: 5,571 coins." in l for l in results["sell_all"]), results
    assert not any("Tax" in l for l in results["sell_all"]), results
    assert alex.count("minecraft:diamond") == 0, alex.run("inventory")
    limits = alex.menu()[50]
    results["limits"] = limits
    assert "No selling limit" in json.dumps(limits), limits
    alex.click(300 + 53)  # close
    assert not alex.menu()
    results["worth"] = alex.say("/worth diamond")
    assert not any("more diamond today" in l for l in results["worth"]), results
    return 2000 + 10 - 15 - 65 + 30 + 5571, results


def invsee(alex, data):
    """Alex, an operator, opens offline Steve's inventory (his saved file): his 5 torches are there; a click takes
    them whole into hers, logged before the move (audit.log) and summed up in staff.log when she closes it."""
    results = {}
    before = alex.count("minecraft:torch")
    alex.say("/invsee Steve")
    time.sleep(2)
    menu = alex.menu()
    results["info"] = menu.get(49)
    assert menu.get(49, {}).get("item") == "minecraft:player_head" and "offline" in json.dumps(menu[49]), menu
    torch = next(slot for slot, e in menu.items() if e["item"] == "minecraft:torch" and slot < 36)
    assert menu[torch]["count"] == 5, menu
    alex.click(300 + torch)
    time.sleep(2)
    assert alex.count("minecraft:torch") == before + 5, alex.run("inventory")
    assert all(e["item"] != "minecraft:torch" for slot, e in alex.menu().items() if slot < 41), alex.menu()
    alex.click(300 + 53)  # close
    time.sleep(1)
    staff = (data / "staff.log").read_text()
    results["staff"] = staff.strip().splitlines()[-1]
    assert "Steve" in staff and "5 torch" in staff, staff
    return results


def market(server, alex, out, data):
    """After the shop, prices moving: her sale moved diamond's one price down, /market's item page buys
    one and the price climbs back the next minute, the 24 h chart comes as a map drawn in her hand, and
    web/market.json has it all. Returns what she spent, and the lines."""
    results = {}

    def worth():
        lines = alex.say("/worth diamond")
        line = next(l for l in lines if "diamond: " in l and "each, to buy or sell." in l)
        return int(re.search(r"diamond: ([\d,]+) coins each", line)[1].replace(",", "")), lines

    # Her 9 diamonds lowered the price as they sold: 200 * e^-0.045 = 191.2 (it drifts back slowly, 6 h half-life).
    price, lines = worth()
    results["after_sale"] = lines
    assert price == 191, lines
    # The move itself is checked above; "Moves" compares with the oldest point of the last hour, which on a history
    # this young can be the first minute's own (0.0%).
    assert any("Moves: " in l and "traded in 24 h: 9" in l for l in lines), lines
    alex.say("/market diamond")
    page = alex.menu()
    results["page"] = page[13]
    assert page[13]["item"] == "minecraft:diamond" and page[48]["item"] == "minecraft:gold_ingot", page
    assert page[19]["item"] == "minecraft:lime_concrete" and page[21]["count"] == 64, page
    assert page[39]["item"] == "minecraft:map", page
    shown = json.dumps(page[13], ensure_ascii=False)
    # Its 24 h move is shown (▼, or "unchanged" on a minute-old history: see "Moves" above).
    assert f"Price: {price} coins each, to buy or sell" in shown and "24 h " in shown, shown
    alex.click(300 + 19)  # buy one
    results["bought"] = alex.chat()[-4:]
    # One diamond moves the price 0.5% as it is bought: 191.7, 192 whole coins.
    paid = 192
    assert any(f"Bought 1 diamond at {paid} coins each: {paid} coins." in l for l in results["bought"]), results
    assert not any("Tax" in l for l in results["bought"]), results
    alex.click(300 + 39)  # the 24 h chart
    assert alex.count("minecraft:filled_map") == 1, alex.run("inventory")
    alex.click(300 + 53)  # close
    alex.run("select", "minecraft:filled_map")
    time.sleep(2)
    alex.run("screenshot", str(Path(out).resolve() / "market-chart.png"))
    # Her purchase pushed it back up at once.
    price2, lines = worth()
    results["after_buy"] = lines
    assert price2 > price, (price, lines)
    # web/market.json is written every minute: wait for the one after her purchase.
    deadline = time.time() + 90
    while True:
        doc = json.loads((data / "web" / "market.json").read_text())
        diamond = next(i for i in doc["items"] if i["id"] == "minecraft:diamond")
        if diamond["volume_24h"] == 10 or time.time() > deadline:
            break
        time.sleep(5)
    results["web"] = {k: diamond[k] for k in ("price", "buy", "sell", "change_1h", "volume_24h", "category")}
    assert doc["schema"] == 1 and doc["moving"] and diamond["category"] == "Mining", results["web"]
    # One price, which buying and selling both use.
    assert 190 < diamond["price"] < 200 and diamond["buy"] == diamond["sell"] == diamond["price"], results["web"]
    assert diamond["volume_24h"] == 10 and diamond["day"]["price"], results["web"]
    return paid, results


def trades(server, alex, steve, port, ctl, resources, root):
    """Every way out of a trade, then one that goes through. Alex: 10 diamonds; Steve: 5 emeralds, 5 torches."""
    results = {}
    items = lambda: (alex.count("minecraft:diamond"), alex.count("minecraft:emerald"),
                     steve.count("minecraft:diamond"), steve.count("minecraft:emerald"))
    assert items() == (10, 0, 0, 5), items()
    # Closing the menu.
    open_trade(alex, steve)
    alex.click(alex.slot_of("minecraft:diamond"))
    shown = steve.menu().get(THEIR_FIRST, {})
    assert (shown.get("item"), shown.get("count")) == ("minecraft:diamond", 10), shown
    assert alex.count("minecraft:diamond") == 10, "offered stacks stay in the inventory"
    alex.run("close")
    time.sleep(1.5)
    results["closed"] = steve.chat()[-2:]
    assert any("Trade cancelled: Alex closed the trade. Nothing was exchanged." in l for l in results["closed"])
    assert not steve.menu(), "the other's menu closes too"
    assert items() == (10, 0, 0, 5), items()
    # A full inventory: Steve can't take the diamonds; nothing moves, the trade stays open, both must confirm again.
    server.console("give Steve minecraft:dirt 2176")
    time.sleep(2)
    open_trade(alex, steve)
    alex.click(alex.slot_of("minecraft:diamond"))
    alex.click(CONFIRM)
    steve.click(CONFIRM)
    results["full"] = alex.chat()[-2:]
    assert any("The trade didn't go through: the second player has no room." in l for l in results["full"]), results
    assert alex.menu().get(45, {}).get("item") == "minecraft:lime_stained_glass_pane", "confirmations cleared"
    assert items() == (10, 0, 0, 5), items()
    # The cancel button.
    steve.click(CANCEL)
    assert any("Trade cancelled: Steve cancelled." in l for l in alex.chat()[-2:])
    server.console("clear Steve minecraft:dirt")
    time.sleep(1)
    # Through: Alex's diamonds for Steve's emeralds and 35 coins. Steve's money offer clears Alex's confirmation.
    open_trade(alex, steve)
    alex.click(alex.slot_of("minecraft:diamond"))
    steve.click(steve.slot_of("minecraft:emerald"))
    alex.click(CONFIRM)
    assert alex.menu()[45]["item"] == "minecraft:lime_concrete"
    assert steve.menu()[52]["item"] == "minecraft:lime_concrete", "Steve sees Alex confirmed"
    steve.say("/trade money 35")
    assert alex.menu()[45]["item"] == "minecraft:lime_stained_glass_pane", "a change clears the confirmation"
    alex.click(CONFIRM)
    steve.click(CONFIRM)
    time.sleep(1)
    results["done"] = alex.chat()[-2:]
    assert any("Trade with Steve done" in l for l in results["done"]), results
    assert items() == (0, 5, 10, 0), items()
    assert any("Balance: 415 coins" in l for l in alex.say("/balance"))
    assert any("Balance: 400 coins" in l for l in steve.say("/balance"))
    # Dying (inventories kept, so a duplicate would show): Alex's death ends it, nothing moved.
    server.console("gamerule keep_inventory true")
    time.sleep(1)
    open_trade(alex, steve)
    alex.click(alex.slot_of("minecraft:emerald"))
    server.console("kill Alex")
    time.sleep(2)
    results["death"] = steve.chat()[-2:]
    assert any("Trade cancelled" in l for l in results["death"]), results
    alex.run("respawn")
    time.sleep(2)
    assert items() == (0, 5, 10, 0), items()
    # Leaving: Steve quits mid-trade; nothing moved, his items are his when he comes back.
    open_trade(alex, steve)
    steve.click(steve.slot_of("minecraft:diamond"))
    steve.quit()
    time.sleep(3)
    results["left"] = alex.chat()[-2:]
    assert any("Trade cancelled" in l for l in results["left"]), results
    assert not alex.menu()
    steve = Client(ctl, resources, "Steve2", root)
    steve.name = "Steve"
    steve.join(port)
    assert items() == (0, 5, 10, 0), items()
    return results, steve


def run(ctl, resources, out):
    if os.environ.get("SMP_PROOF") == "casino":
        # The casino's own proof (proof_casino.py), apart: it plays its games for a few minutes.
        import importlib.util
        spec = importlib.util.spec_from_file_location("smp_proof_casino", Path(__file__).with_name("proof_casino.py"))
        casino = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(casino)
        return casino.run(ctl, resources, out)
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    game = Path(ctl).with_name("horizon-client")
    root = Path(tempfile.mkdtemp(prefix="smp-proof-"))
    port = free_port()
    server = Server(game, resources, root / "server", port)
    alex = Client(ctl, resources, "Alex", root)
    steve = Client(ctl, resources, "Steve", root)
    results = {}
    try:
        data = server.folder / "world" / "horizon-smp"
        data.mkdir(parents=True)
        (data / "config.json").write_text(json.dumps({"gambling": {"enabled": True, "cooldown_seconds": 20,
                                                                         "lottery_seconds": 30},
                                                      # Quick teleports and an instant reward (the reward's
                                                      # delay has unit tests).
                                                      "places": {"warmup_seconds": 2, "cooldown_seconds": 0},
                                                      # Paid once, after the restart, not to change the sums.
                                                      "presence": {"daily_reward": 0, "reward_after_minutes": 0},
                                                      # Steve plays a few minutes before his nickname.
                                                      "nicks": {"min_playtime_minutes": 1},
                                                      # Fixed prices for the first run's sums; the
                                                      # moving market is on after the restart.
                                                      "market": {"dynamic": {"enabled": False}},
                                                      # The casino's pot gets no float here (its own proof,
                                                      # proof_casino.py, covers it): the sums stay.
                                                      "casino": {"float": 0}}))
        # A world from before 0.30, with a Treasury: its pool is dropped at the first start.
        (data / "economy.json").write_text(json.dumps({"accounts": {}, "pools": {"treasury": 5000}}))
        # A nickname an older version let through, taken off at the first start.
        (data / "nicks.json").write_text(json.dumps({"nicks": {"00000000-0000-0000-0000-00000000abcd": {
            "nick": "N0tch", "changed": 0}}}))
        server.start()
        # No monster may end a step early: a slime once killed Steve on the flat world.
        for line in ("difficulty peaceful", "gamerule spawn_monsters false", "kill @e[type=minecraft:slime]"):
            server.console(line)
        alex.join(port)
        steve.join(port)
        time.sleep(2)
        config = json.loads((data / "config.json").read_text())
        results["config_keys"] = sorted(config)
        assert results["config_keys"] == ["auction", "board", "casino", "claims", "currency", "gambling", "market", "nicks", "places", "presence", "report", "shop",
                                          "starting_balance", "web"], config
        results["welcome"] = steve.chat()
        assert any("Welcome to the server, Steve!" in l for l in results["welcome"]), results["welcome"]
        assert any("1. Be respectful to everyone." in l for l in results["welcome"]), results["welcome"]
        assert any("Steve joined for the first time" in l for l in alex.chat()), alex.chat()
        assert config["gambling"] == {"enabled": True, "min_bet": 1, "max_bet": 1_000_000_000_000, "cooldown_seconds": 20,
                                      "offer_seconds": 60, "ticket_price": 10,
                                      "max_tickets": 0, "lottery_seconds": 30}, config
        # One price, to buy and to sell.
        assert config["market"]["items"]["minecraft:diamond"] == {"buy": 200, "sell": 200}, config
        server.console("op Alex")
        time.sleep(1)
        results["grant"] = alex.say("/eco give Alex 500 proof funding")
        assert any("Alex gave 500 coins to Alex: proof funding." in l for l in results["grant"]), results
        assert any("Alex gave 500 coins to Alex: proof funding." in l for l in steve.chat()[-3:]), "everyone reads it"
        results["balance"] = alex.say("/balance")
        assert any("Balance: 500 coins" in l for l in results["balance"]), results
        results["pay"] = alex.say("/pay Steve 120")
        assert any("You paid 120 coins to Steve. Balance: 380 coins" in l for l in results["pay"]), results
        results["paid"] = steve.chat()[-3:]
        assert any("Alex paid you 120 coins." in l for l in results["paid"]), results
        # Payments require sufficient funds, with no daily receiving cap.
        results["refused"] = [alex.say(l) for l in ("/pay Steve 1500", "/pay Steve 9999", "/pay Alex 1",
                                                    "/pay Nobody 1")]
        for lines, error in zip(results["refused"], ("You only have 380 coins.",
                                                     "You only have 380 coins.",
                                                     "You can't pay yourself.",
                                                     "No player named Nobody has an account.")):
            assert any(error in l for l in lines), results["refused"]
        results["eco_refused"] = steve.say("/eco give Steve 1000 because")
        assert not any("gave" in l for l in results["eco_refused"]), results
        # The parts taken out: their commands are gone.
        for line in ("/treasury", "/bank", "/jobs", "/futures", "/eco taxes"):
            results[line] = steve.say(line)
            refused = ("Unknown or incomplete command", "Incorrect argument", "Usage", "Only operators")
            assert any(any(r in l for r in refused) for l in results[line]), results
        results["baltop"] = steve.say("/baltop")
        assert any("1. Alex: 380 coins" in l for l in results["baltop"]), results
        assert any("2. Steve: 120 coins" in l for l in results["baltop"]), results
        server.console("give Steve minecraft:diamond 1")
        server.console("give Steve minecraft:wheat 60")
        time.sleep(2)
        results["worth"] = steve.say("/worth diamond")
        assert any("diamond: 200 coins each, to buy or sell." in l for l in results["worth"]), results
        results["sell_wheat"] = steve.say("/sell wheat")
        assert any("Sold 60 wheat at 2 coins each: 120 coins." in l for l in results["sell_wheat"]), results
        results["sell_hand"] = steve.say("/sell hand")
        assert any("Sold 1 diamond at 200 coins each: 200 coins." in l for l in results["sell_hand"]), results
        results["buy"] = steve.say("/buy torch 5")
        assert any("Bought 5 torch at 1 coins each: 5 coins." in l for l in results["buy"]), results
        time.sleep(1)
        counts = steve.run("inventory").get("counts", {})
        results["steve_items"] = counts
        have = lambda item: counts.get(f"minecraft:{item}", counts.get(item, 0))
        assert (have("diamond"), have("wheat"), have("torch")) == (0, 0, 5), counts
        server.console("gamemode creative Alex")
        time.sleep(1)
        results["creative"] = alex.say("/sell all")
        assert any("only with players in survival or adventure mode" in l for l in results["creative"]), results
        server.console("gamemode survival Alex")
        server.console("give Alex minecraft:diamond 10")
        server.console("give Steve minecraft:emerald 5")
        time.sleep(2)
        results["trades"], steve = trades(server, alex, steve, port, ctl, resources, root)
        # Before the coinflips: their last offer must still wait when the server stops.
        results["places"] = places(server, alex, steve)
        results["claims"] = claims(server, alex, steve)
        results["world_guard"] = world_guard(server, alex, steve)
        results["board"] = board(alex, steve, out)
        results["polish"] = polish(server, alex, steve, out)
        results["presence"] = presence(server, alex, steve)
        results["nicks"] = nicks(server, alex, steve, out)
        results["coinflip"], alex_wins = coinflips(alex, steve)
        results["lottery"], alex_lotto = lottery(alex, steve, alex_wins)
        alex_after = 415 + (100 if alex_wins else -100) + (20 if alex_lotto else -30)
        # Steve's 50 is still waiting: the server stops with both on.
        lines = server.stop()
        steve.quit()
        alex.quit()
        assert not any("panicked" in l or "teleport failed" in l for l in lines), lines[-40:]
        saved = json.loads((data / "places.json").read_text())
        assert sorted(saved["warps"]) == ["shop"] and saved["spawn"] and len(saved["homes"]) == 1, saved
        saved = json.loads((data / "board.json").read_text())
        assert len(saved["hidden"]) == 1, saved
        saved = json.loads((data / "claims.json").read_text())
        assert len(saved["chunks"]) == 1 and len(saved["trust"]) == 1, saved
        economy = json.loads((data / "economy.json").read_text())
        balances = {a["name"]: a["balance"] for a in economy["accounts"].values()}
        results["economy.json"] = economy
        held = sum(economy.get("held", {}).values())
        assert balances["Alex"] == alex_after and sum(balances.values()) + held == 815, economy
        audit = [json.loads(l) for l in (data / "audit.log").read_text().splitlines()]
        results["audit"] = [a["kind"] for a in audit]
        assert results["audit"] == ["open", "open", "eco_give", "pay", "sell 60 minecraft:wheat",
                                    "sell 1 minecraft:diamond", "buy 5 minecraft:torch", "trade", "trade_failed",
                                    "trade", "trade_money", "pay", "pay", "coinflip_stake", "coinflip_stake", "coinflip_win",
                                    "coinflip_stake", "lottery_ticket", "lottery_ticket", "lottery_win"] + (
                                    ["coinflip_cancel"] * (held == 0)), results["audit"]
        swap = [a for a in audit if a["kind"] == "trade"][-1]
        assert (swap["a_items"], swap["b_items"], swap["b_money"]) == (
            [{"item": "minecraft:diamond", "count": 10, "plain": True}],
            [{"item": "minecraft:emerald", "count": 5, "plain": True}], 35), swap
        # The daily reward, on: Alex gets it when she joins again.
        config = json.loads((data / "config.json").read_text())
        config["presence"]["daily_reward"] = 7
        config["market"]["dynamic"]["enabled"] = True
        # The web pages go to the website through Kira: here to a stand-in, with a token.
        receiver = Receiver()
        config["web"] = {"url": receiver.url, "token": "proof-token"}
        (data / "config.json").write_text(json.dumps(config))
        server.start()
        alex = Client(ctl, resources, "Alex2", root)
        alex.name = "Alex"
        alex.join(port)
        results["after_restart"] = alex.say("/balance")
        assert any(f"Balance: {alex_after + 7} coins" in l for l in results["after_restart"]), results
        assert any("Daily reward: 7 coins." in l for l in alex.chat()), alex.chat()
        assert not any("joined for the first time" in l for l in alex.chat()), alex.chat()
        added, results["shop"] = shop(server, alex)
        spent, results["market"] = market(server, alex, out, data)
        results["invsee"] = invsee(alex, data)
        sent = {path: (auth, doc) for path, auth, doc in receiver.got}
        results["pushed"] = sorted(sent)
        assert {"/smp/ingest/market.json", "/smp/ingest/economy.json"} <= set(sent), receiver.got[:2]
        assert all(auth == "Bearer proof-token" for auth, _ in sent.values()), [a for a, _ in sent.values()]
        assert sent["/smp/ingest/market.json"][1]["schema"] == 1 and sent["/smp/ingest/market.json"][1]["items"]
        receiver.server.shutdown()
        added -= spent
        alex.quit()
        server.stop()
        book = json.loads((data / "market_prices.json").read_text())
        assert book["items"]["minecraft:diamond"]["minutes"], "the prices' history is written when the world closes"
        economy = json.loads((data / "economy.json").read_text())
        total = sum(a["balance"] for a in economy["accounts"].values())
        assert "held" not in economy and total == 815 + 7 + added, economy
        # The old Treasury pool is gone; no fee or tax took anything.
        assert "treasury" not in economy.get("pools", {}), economy.get("pools")
        audit = [json.loads(l) for l in (data / "audit.log").read_text().splitlines()]
        took = [a for a in audit if a["kind"] == "invsee_take"]
        assert [(a["item"], a["count"], a["inventory"]) for a in took] == [("minecraft:torch", 5, "inventory")], took
        kinds = [a["kind"] for a in audit if not a["kind"].startswith("invsee_")]
        shop_kinds = ["eco_give", "buy 1 minecraft:iron_ingot", "buy 64 minecraft:cobblestone",
                      "sell 1 minecraft:iron_ingot", "sell 1 minecraft:iron_ingot", "sell 30 minecraft:diamond", "buy 1 minecraft:diamond"]
        assert kinds[-len(shop_kinds):] == shop_kinds, kinds[-8:]
        kinds = kinds[:-len(shop_kinds)]
        assert kinds[-2] in ("held_returned", "coinflip_cancel") and kinds[-1] == "daily_reward", kinds[-3:]
        results["stake_back"] = kinds[-2]
        sold = json.loads((data / "market.json").read_text())["sold"]
        # Daily sales are recorded for statistics; payments are unrestricted.
        assert sorted((s["coins"], s["items"]) for s in sold.values()) == [
            (320, {"minecraft:diamond": 1, "minecraft:wheat": 60}),
            (5601, {"minecraft:diamond": 30, "minecraft:iron_ingot": 2})], sold
        print(json.dumps({"proof": "smp", "ok": True, **results}, indent=2))
    finally:
        (out / "server.log").write_text("\n".join(server.lines))
        for c in (alex, steve):
            c.quit()
        if server.__dict__.get("process") and server.process.poll() is None:
            server.process.kill()
        shutil.rmtree(root, ignore_errors=True)
