"""The casino on a real dedicated server: the built game runs `--dedicated` with the mod, two real clients (Alex and
Steve, `horizon-ctl`) join it and play every game through the server's chest menus, as a stock client would.
Run it with `SMP_PROOF=casino python3 native/tools/native.py proof --resources PATH smp` (proof.py hands over to it);
`SMP_PROOF_HEADLESS=1` starts the clients without a GPU (no screenshots).

Checked from the clients' own menus, chat, titles and action bar, and from the files the server wrote:

- the pot gets its float at the first start (made once), and `/casino help` shows every game's odds;
- `/casino` opens the lobby (its title, the four games, the pot); a click opens a game;
- roulette: a bet on Red, the stake raised, the wheel turns (several different pictures while it spins), the round
  is paid and the table shows the result; a second spin with the menu closed mid-spin is still paid, once;
- slots: a double click pulls once; the reels scroll; closing the menu doesn't stop the pull;
- crash: both players bet in the same round; Steve leaves mid-flight (cashed out or crashed, his bet settled once),
  Alex cashes out or crashes;
- blackjack: a hand dealt and stood (or settled at once on a blackjack);
- the bet selector reaches the economy numeric range despite legacy limits;
- after the server stops: every bet held is settled exactly once in audit.log, nothing is left held, and the money
  in the game (balances + pools) is the float and the operator's two `/eco give`s; casino.json holds the day's totals; no
  console command the casino ran was refused.
"""
import importlib.util
import json
import os
import re
import shutil
import tempfile
import time
from pathlib import Path

_spec = importlib.util.spec_from_file_location("smp_proof_harness", Path(__file__).with_name("proof.py"))
harness = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(harness)

FLOAT = 20_000
GIVEN = 2 * 3000
NAME = re.compile(r'"?text"?\s*:\s*"((?:[^"\\]|\\.)*)"')
GAMES = ("casino_roulette", "casino_slots", "casino_crash", "casino_blackjack")


class Client(harness.Client):
    def join(self, port):
        mode = "--headless" if os.environ.get("SMP_PROOF_HEADLESS") else "--offscreen"
        self.run("launch", mode, "--resources", self.resources)
        self.run("connect", f"127.0.0.1:{port}", self.name)
        self.run("wait-for", "loaded", "--timeout", 300)
        time.sleep(2)

    def cells(self):
        """The open casino menu: slot -> (item, name)."""
        out = {}
        for slot, e in self.menu().items():
            m = NAME.search(e.get("components", ""))
            out[slot] = (e["item"], m.group(1) if m else "")
        return out

    def name_at(self, slot):
        return self.cells().get(slot, ("", ""))[1]

    def title(self):
        return "".join(s["text"] for s in self.screen().get("title", []))

    def quick(self, slot):
        self.run("click-slot", 300 + slot)

    def shot(self, out, name):
        if not os.environ.get("SMP_PROOF_HEADLESS"):
            self.run("screenshot", str(Path(out).resolve() / name), check=False)

    def wait_name(self, slot, text, timeout=30):
        end = time.time() + timeout
        while time.time() < end:
            name = self.name_at(slot)
            if text in name:
                return name
            time.sleep(0.25)
        raise AssertionError(f"{self.name}: slot {slot} never showed {text!r}: {self.cells()}")


def shown(c, seconds=3):
    """The titles and action bar lines `c` sees over the next `seconds`."""
    seen = []
    end = time.time() + seconds
    while time.time() < end:
        m = c.run("messages", 1)
        for line in (m.get("title"), m.get("subtitle"), m.get("actionbar")):
            if line and line not in seen:
                seen.append(line)
        time.sleep(0.2)
    return seen


def audit(data):
    return [json.loads(l) for l in (data / "audit.log").read_text().splitlines()]


def settled(data):
    return [a for a in audit(data) if a["kind"] in GAMES]


def roulette(alex, out):
    r = {}
    alex.say("/casino roulette")
    assert alex.title() == "Casino · Roulette", alex.title()
    alex.click(300 + 18)  # Red
    assert "Red" in alex.name_at(40), alex.cells()
    for _ in range(3):
        alex.click(300 + 49)  # more
    r["bet"] = alex.name_at(48)
    pictures = []
    alex.quick(53)
    alex.wait_name(53, "turns", 10)
    shot, started = False, time.time()
    end = time.time() + 30
    # Until the wheel stops: each picture the menu shows, and the result's title or action bar.
    while time.time() < end:
        cells = alex.cells()
        if "SPIN" in cells.get(53, ("", ""))[1]:
            break
        pictures.append(tuple(cells.get(s) for s in range(18)))
        if not shot:
            alex.shot(out, "roulette-spin.png")
            shot = True
    r["pictures"] = len(set(pictures))
    r["samples"], r["spin_seconds"] = len(pictures), round(time.time() - started, 1)
    assert r["pictures"] >= 3, "the wheel didn't turn"
    r["result"] = alex.name_at(40)
    r["shown"] = shown(alex)
    assert r["shown"], "no title or action bar for the result"
    alex.shot(out, "roulette-result.png")
    # A second spin, the menu closed at once: paid all the same.
    time.sleep(2.5)
    alex.quick(53)
    time.sleep(0.4)
    alex.run("close")
    time.sleep(6)
    return r


def slots(alex, out):
    r = {}
    alex.say("/casino slots")
    assert alex.title() == "Casino · Slots", alex.title()
    paytable = [alex.name_at(s) for s in range(36, 44)]
    assert "3 Nether Star: 500x" in paytable and "94.2% paid back on average" in paytable, paytable
    alex.quick(53)
    alex.quick(53)  # the double click
    time.sleep(0.8)
    alex.shot(out, "slots-spin.png")
    r["turning"] = alex.name_at(53)
    assert "turn" in r["turning"], r
    alex.wait_name(53, "PULL")
    r["line"] = alex.name_at(4)
    assert "won" in r["line"] or "no luck" in r["line"], r
    time.sleep(2.5)
    alex.quick(53)
    time.sleep(0.3)
    alex.run("close")
    time.sleep(4)
    return r


def crash(alex, steve, out):
    r = {}
    for c in (alex, steve):
        c.say("/casino crash")
        assert c.title() == "Casino · Crash", c.title()
    alex.quick(53)
    time.sleep(0.5)
    steve.quick(53)
    r["in"] = [c.wait_name(53, "Your bet is in", 8) for c in (alex, steve)]
    end = time.time() + 20
    while time.time() < end and not alex.name_at(38).startswith(("Flying", "Crashed")):
        time.sleep(0.25)
    r["status"] = alex.name_at(38)
    alex.shot(out, "crash-flight.png")
    steve.quit()  # leaves mid-flight
    time.sleep(1.5)
    button = alex.name_at(53)
    if button.startswith("CASH OUT"):
        alex.quick(53)
        r["alex"] = "cashed out"
    else:
        r["alex"] = button
    r["shown"] = shown(alex)
    # The round ends (a crash, at most 100x: wait for it to show).
    end = time.time() + 120
    while time.time() < end and not alex.name_at(38).startswith(("Crashed", "No round")):
        time.sleep(1)
    r["end"] = alex.name_at(38)
    alex.run("close")
    return r


def blackjack(alex, out):
    r = {}
    alex.say("/casino blackjack")
    assert alex.title() == "Casino · Blackjack", alex.title()
    alex.click(300 + 53)
    alex.shot(out, "blackjack-hand.png")
    r["dealt"] = alex.cells().get(40)
    if r["dealt"] and r["dealt"][1] == "STAND":
        alex.click(300 + 40)
    time.sleep(3)
    r["end"] = alex.name_at(4)
    # The most a bet may be.
    alex.run("click-slot", 300 + 49, "shift")
    time.sleep(1)
    r["max"] = alex.name_at(48)
    assert r["max"] == "Bet: 1,000,000,000,000 coins", r
    alex.run("close")
    return r


def run(ctl, resources, out):
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    game = Path(ctl).with_name("horizon-client")
    root = Path(tempfile.mkdtemp(prefix="smp-casino-proof-"))
    port = harness.free_port()
    server = harness.Server(game, resources, root / "server", port)
    alex = Client(ctl, resources, "Alex", root)
    steve = Client(ctl, resources, "Steve", root)
    results = {}
    try:
        data = server.folder / "world" / "horizon-smp"
        data.mkdir(parents=True)
        (data / "config.json").write_text(json.dumps({
            "gambling": {"enabled": True}, "starting_balance": 0, "presence": {"daily_reward": 0},
            "casino": {"cooldown_ms": 500, "float": FLOAT}}))
        server.start()
        for line in ("difficulty peaceful", "gamerule spawn_monsters false"):
            server.console(line)
        alex.join(port)
        steve.join(port)
        server.console("op Alex")
        time.sleep(2)
        for who in ("Alex", "Steve"):
            assert any(f"Alex gave 3,000 coins to {who}: casino proof." in l for l in alex.say(f"/eco give {who} 3000 casino proof"))
        results["help"] = alex.say("/casino help")
        text = "\n".join(results["help"])
        for want in ("no daily win/loss limits or cooldown", "pot (20,000 coins now)", "European", "94.2%", "96%", "3 to 2"):
            assert want in text, (want, results["help"])
        alex.say("/casino")
        assert alex.title() == "Casino", alex.title()
        lobby = alex.cells()
        results["lobby"] = {s: lobby.get(s) for s in (19, 21, 23, 25, 42)}
        assert lobby[19][0] == "minecraft:red_concrete" and "Roulette" in lobby[19][1], lobby
        assert lobby[42][1] == "The casino's pot: 20,000 coins", lobby
        alex.shot(out, "lobby.png")
        alex.click(300 + 21)
        assert alex.title() == "Casino · Slots", alex.title()
        alex.run("close")
        results["roulette"] = roulette(alex, out)
        results["slots"] = slots(alex, out)
        results["blackjack"] = blackjack(alex, out)
        results["crash"] = crash(alex, steve, out)
        time.sleep(3)
        lines = server.stop()
        alex.quit()
        refused = [l for l in lines if "Unknown or incomplete command" in l or "Incorrect argument" in l
                   or "panicked" in l]
        assert not refused, refused
        log = audit(data)
        bets = [a for a in log if a["kind"] == "casino_bet"]
        ends = [a for a in log if a["kind"] in GAMES or a["kind"] == "casino_void"]
        results["audit"] = [a["kind"] for a in log if a["kind"].startswith("casino")]
        # Roulette 2, slots 2 (the double click pulled once), blackjack 1, crash 2; a double adds a second hold.
        counts = {g: sum(a["kind"] == g for a in ends) for g in GAMES}
        results["rounds"] = counts
        assert counts == {"casino_roulette": 2, "casino_slots": 2, "casino_blackjack": 1, "casino_crash": 2}, counts
        assert len(bets) >= 7, bets
        economy = json.loads((data / "economy.json").read_text())
        results["economy"] = economy
        held = sum(economy.get("held", {}).values())
        money = sum(a["balance"] for a in economy["accounts"].values()) + held + sum(economy["pools"].values())
        assert held == 0 and money == FLOAT + GIVEN, economy
        ledger = json.loads((data / "casino.json").read_text())
        results["casino.json"] = ledger
        assert sum(t["rounds"] for t in ledger["totals"].values()) == 7, ledger
        print(json.dumps({"proof": "smp-casino", "ok": True, **results}, indent=2, ensure_ascii=False))
    except BaseException:
        print(json.dumps({"proof": "smp-casino", "ok": False, **results}, indent=2, ensure_ascii=False, default=str))
        if (server.folder / "world" / "horizon-smp" / "audit.log").exists():
            print("audit:", [a["kind"] for a in audit(server.folder / "world" / "horizon-smp")])
        raise
    finally:
        (out / "server.log").write_text("\n".join(server.lines))
        for c in (alex, steve):
            c.quit()
        if server.__dict__.get("process") and server.process.poll() is None:
            server.process.kill()
        if not os.environ.get("SMP_PROOF_KEEP"):
            shutil.rmtree(root, ignore_errors=True)
