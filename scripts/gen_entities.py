#!/usr/bin/env python3
"""Generate config/entities.toml from data/data.json.
Robust Unit array parsing with independent tracking of square bracket depth (for the Unit array) and
curly brace depth (for objects). Extract id, name, radius; map icon via overrides or direct name.webp.
"""
import pathlib, sys, re
from typing import List, Tuple

DATA_PATH = pathlib.Path('data/data.json')
OUT_PATH = pathlib.Path('config/entities.toml')
ICONS_DIR = pathlib.Path('assets/icons')

OVERRIDE = {
    'InfestorTerran': 'InfestedTerran.webp',
    'HellionTank': 'Hellbat.webp',
    'BanelingCocoon': 'Baneling.webp',
    'BroodLordCocoon': 'BroodLord.webp',
    'OverlordCocoon': 'Overlord.webp',
    'TransportOverlordCocoon': 'Overlord.webp',
    'OracleStasisTrap': 'StasisWard.webp',
    'PylonOvercharged': 'Pylon.webp',
    'ShieldBattery': 'ShieldBattery.webp',
    'ObserverSiegeMode': 'Observer.webp',
    'OverseerSiegeMode': 'Overseer.webp',
    'ThorAALance': 'Thor.webp',
}

UNIT_ICON_MAP = {
    # Protoss Units
    "Zealot": "Zealot.webp",
    "Stalker": "Stalker.webp",
    "Sentry": "Sentry.webp",
    "Adept": "Adept.webp",
    "HighTemplar": "HighTemplar.webp",
    "DarkTemplar": "DarkTemplar.webp",
    "Immortal": "Immortal.webp",
    "Colossus": "Colossus.webp",
    "Disruptor": "Disruptor.webp",
    "Archon": "Archon.webp",
    "Observer": "Observer.webp",
    "ObserverSiegeMode": "Observer.webp",          # No separate icon; reuse
    "WarpPrism": "WarpPrism.webp",
    "Oracle": "Oracle.webp",
    "Phoenix": "Phoenix.webp",
    "VoidRay": "VoidRay.webp",
    "Carrier": "Carrier.webp",
    "Tempest": "Tempest.webp",
    "Mothership": "Mothership.webp",
    "MothershipCore": "MothershipCore.webp",
    "ShieldBattery": "ShieldBattery.webp",
    "Probe": "Probe.webp",

    # Protoss Buildings / Tech
    "Nexus": "Nexus.webp",
    "Pylon": "Pylon.webp",
    "PylonOvercharged": "Pylon.webp",
    "Assimilator": "Assimilator.webp",
    "Gateway": "Gateway.webp",
    "WarpGate": "WarpGate.webp",
    "CyberneticsCore": "CyberneticsCore.webp",
    "Forge": "Forge.webp",
    "TwilightCouncil": "TwilightCouncil.webp",
    "TemplarArchives": "TemplarArchives.webp",
    "DarkShrine": "DarkShrine.webp",
    "RoboticsFacility": "RoboticsFacility.webp",
    "RoboticsBay": "RoboticsBay.webp",
    "Stargate": "Stargate.webp",
    "FleetBeacon": "FleetBeacon.webp",
    "PhotonCannon": "PhotonCannon.webp",
    "ShieldBattery": "ShieldBattery.webp",
    "OracleStasisTrap": "StasisWard.webp",         # Internal ability object
    "StasisWard": "StasisWard.webp",

    # Terran Units
    "SCV": "SCV.webp",
    "Marine": "Marine.webp",
    "Marauder": "Marauder.webp",
    "Reaper": "Reaper.webp",
    "Ghost": "Ghost.webp",
    "Hellion": "Hellion.webp",
    "Hellbat": "Hellbat.webp",
    "HellionTank": "Hellbat.webp",                 # Morph alias
    "Cyclone": "Cyclone.webp",
    "WidowMine": "WidowMine.webp",
    "SiegeTank": "SiegeTank.webp",
    "SiegeTankSieged": "SiegeTankSieged.webp",
    "Thor": "Thor.webp",
    "ThorAALance": "Thor.webp",                    # Mode alias
    "Viking": "Viking.webp",                       # Generic (fallback)
    "VikingAssault": "VikingAssault.webp",
    "VikingFighter": "VikingFighter.webp",
    "Medivac": "Medivac.webp",
    "Liberator": "Liberator.webp",
    "LiberatorAG": "LiberatorAG.webp",
    "Raven": "Raven.webp",
    "Banshee": "Banshee.webp",
    "Battlecruiser": "Battlecruiser.webp",
    "MULE": "MULE.webp",
    "PointDefenseDrone": "PointDefenseDrone.webp",

    # Terran Buildings / Addons
    "CommandCenter": "CommandCenter.webp",
    "OrbitalCommand": "OrbitalCommand.webp",
    "PlanetaryFortress": "PlanetaryFortress.webp",
    "SupplyDepot": "SupplyDepot.webp",
    "Refinery": "Refinery.webp",
    "Barracks": "Barracks.webp",
    "Factory": "Factory.webp",
    "Starport": "Starport.webp",
    "EngineeringBay": "EngineeringBay.webp",
    "Armory": "Armory.webp",
    "FusionCore": "FusionCore.webp",
    "GhostAcademy": "GhostAcademy.webp",
    "SensorTower": "SensorTower.webp",
    "MissileTurret": "MissileTurret.webp",
    "Bunker": "Bunker.webp",
    "Reactor": "Reactor.webp",
    "TechLab": "TechLab.webp",
    "BarracksReactor": "BarracksReactor.webp",
    "BarracksTechLab": "BarracksTechLab.webp",
    "FactoryReactor": "FactoryReactor.webp",
    "FactoryTechLab": "FactoryTechLab.webp",
    "StarportReactor": "StarportReactor.webp",
    "StarportTechLab": "StarportTechLab.webp",

    # Zerg Units
    "Drone": "Drone.webp",
    "Larva": "Larva.webp",
    "Overlord": "Overlord.webp",
    "Overseer": "Overseer.webp",
    "Queen": "Queen.webp",
    "Zergling": "Zergling.webp",
    "Baneling": "Baneling.webp",
    "BanelingCocoon": "Baneling.webp",
    "Roach": "Roach.webp",
    "Ravager": "Ravager.webp",
    "Hydralisk": "Hydralisk.webp",
    "Lurker": "Lurker.webp",
    "Mutalisk": "Mutalisk.webp",
    "Corruptor": "Corruptor.webp",
    "Viper": "Viper.webp",
    "SwarmHost": "SwarmHost.webp",
    "Infestor": "Infestor.webp",
    "InfestorTerran": "InfestedTerran.webp",
    "InfestedTerran": "InfestedTerran.webp",
    "Ultralisk": "Ultralisk.webp",
    "BroodLord": "BroodLord.webp",
    "Broodling": "Broodling.webp",
    "Changeling": "Changeling.webp",
    "CreepTumor": "CreepTumor.webp",
    "CreepTumorQueen": "CreepTumorQueen.webp",
    "LurkerDen": "LurkerDen.webp",                 # (Actually a structure)
    "OverlordCocoon": "Overlord.webp",
    "TransportOverlordCocoon": "Overlord.webp",

    # Zerg Buildings / Tech
    "Hatchery": "Hatchery.webp",
    "Lair": "Lair.webp",
    "Hive": "Hive.webp",
    "SpawningPool": "SpawningPool.webp",
    "BanelingNest": "BanelingNest.webp",
    "RoachWarren": "RoachWarren.webp",
    "HydraliskDen": "HydraliskDen.webp",
    "LurkerDen": "LurkerDen.webp",
    "Spire": "Spire.webp",
    "GreaterSpire": "GreaterSpire.webp",
    "UltraliskCavern": "UltraliskCavern.webp",
    "InfestationPit": "InfestationPit.webp",
    "EvolutionChamber": "EvolutionChamber.webp",
    "SpineCrawler": "SpineCrawler.webp",
    "SporeCrawler": "SporeCrawler.webp",
    "NydusNetwork": "NydusNetwork.webp",
    "NydusWorm": "NydusWorm.webp",

    # Shared / Special / Ability / Misc
    "AutoTurret": "AutoTurret.webp",
    "OracleStasisTrap": "StasisWard.webp",
    "StasisWard": "StasisWard.webp",
    "PlanetaryFortress": "PlanetaryFortress.webp",  # (already listed)
    "ShieldBattery": "ShieldBattery.webp",          # (already listed)
    "WarpGate": "WarpGate.webp",                    # (already listed)
    "WarpPrism": "WarpPrism.webp",
}

UNIT_ICON_MAP = {k: f"icons/{v}" for k, v in UNIT_ICON_MAP.items()}

import json
def deserialize(fname: str):
    with open(fname, 'r') as f:
        python_object = json.load(f)
    return python_object


def get_icon(unit_name: str) -> str:
    # Priority: OVERRIDE, UNIT_ICON_MAP, fallback to name
    if unit_name in OVERRIDE:
        return f"icons/{OVERRIDE[unit_name]}"
    if unit_name in UNIT_ICON_MAP:
        return UNIT_ICON_MAP[unit_name]
    return f"icons/{unit_name}.webp"

def toml_escape(s: str) -> str:
    # Escape for TOML basic string
    return s.replace('"', '\\"')

def generate_toml(units) -> str:
    lines = []
    for unit in units:
        unit_id = unit.get("id")
        name = unit.get("name")
        radius = unit.get("radius")
        if not (unit_id and name and radius is not None):
            continue  # skip incomplete
        icon = get_icon(name)
        lines.append(f'[[entity]]')
        lines.append(f'id = {unit_id}')
        lines.append(f'name = "{toml_escape(name)}"')
        lines.append(f'radius = {radius}')
        lines.append(f'icon = "{icon}"')
        lines.append('')  # blank line between entities
    return '\n'.join(lines)

def main():
    root = deserialize(DATA_PATH);
    units = root["Unit"]
    print(f"Generating entities.toml for {len(units)} units...");
    toml = generate_toml(units)
    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(toml)
    print(f"Wrote {OUT_PATH}")

if __name__ == "__main__":
    main()
