python3 - <<'PY'
    'InfestorTerran':'InfestedTerran.webp',
    'VikingAssault':'VikingAssault.webp',
    'VikingFighter':'VikingFighter.webp',
    'SiegeTankSieged':'SiegeTankSieged.webp',
    'SiegeTank':'SiegeTank.webp',
    'HellionTank':'Hellbat.webp',
> import os, re, json
    'RoboticsFacility':'RoboticsFacility.webp',
    'RoboticsBay':'RoboticsBay.webp',
> # Collect existing unit ids in entities.toml
    'Lurker':'Lurker.webp',
> existing_ids=set()
    'SwarmHost':'SwarmHost.webp',
    'MULE':'MULE.webp',
> with open('config/entities.toml','r',encoding='utf-8') as f:
    'Refinery':'Refinery.webp',
    'EngineeringBay':'EngineeringBay.webp',
>     for line in f:
    for line in f:
        if '"Unit":[' in line:
>         m=re.match(r'\[unit\.(\d+)\]', line.strip())
>         if m:
>             existing_ids.add(int(m.group(1)))
> icons=set(os.listdir('assets/icons'))
> # Special icon name mapping
> special_map={
>     'InfestorTerran':'InfestedTerran.webp',
>     'VikingAssault':'VikingAssault.webp',
>     'VikingFighter':'VikingFighter.webp',
>     'SiegeTankSieged':'SiegeTankSieged.webp',
>     'SiegeTank':'SiegeTank.webp',
>     'HellionTank':'Hellbat.webp',
>     'Hellion':'Hellion.webp',
>     'SCV':'SCV.webp',
>     'PointDefenseDrone':'PointDefenseDrone.webp',
>     'MothershipCore':'MothershipCore.webp',
>     'Observer':'Observer.webp',
>     'WarpPrism':'WarpPrism.webp',
>     'WarpGate':'WarpGate.webp',
>     'HydraliskDen':'HydraliskDen.webp',
>     'SpawningPool':'SpawningPool.webp',
>     'SpineCrawler':'SpineCrawler.webp',
>     'SporeCrawler':'SporeCrawler.webp',
>     'UltraliskCavern':'UltraliskCavern.webp',
>     'CyberneticsCore':'CyberneticsCore.webp',
>     'RoboticsFacility':'RoboticsFacility.webp',
>     'RoboticsBay':'RoboticsBay.webp',
>     'TwilightCouncil':'TwilightCouncil.webp',
>     'TemplarArchives':'TemplarArchives.webp',
>     'FusionCore':'FusionCore.webp',
>     'GhostAcademy':'GhostAcademy.webp',
>     'DarkShrine':'DarkShrine.webp',
>     'BanelingNest':'BanelingNest.webp',
>     'GreaterSpire':'GreaterSpire.webp',
>     'InfestationPit':'InfestationPit.webp',
>     'LurkerDen':'LurkerDen.webp',
>     'Ultralisk':'Ultralisk.webp',
>     'PlanetaryFortress':'PlanetaryFortress.webp',
>     'OrbitalCommand':'OrbitalCommand.webp',
>     'Overlord':'Overlord.webp',
>     'Overseer':'Overseer.webp',
>     'Queen':'Queen.webp',
>     'BroodLord':'BroodLord.webp',
>     'Broodling':'Broodling.webp',
>     'Larva':'Larva.webp',
>     'Drone':'Drone.webp',
>     'Ravager':'Ravager.webp',
>     'Lurker':'Lurker.webp',
>     'Hatchery':'Hatchery.webp',
>     'Lair':'Lair.webp',
>     'Hive':'Hive.webp',
>     'Mutalisk':'Mutalisk.webp',
>     'Corruptor':'Corruptor.webp',
>     'Viper':'Viper.webp',
>     'Baneling':'Baneling.webp',
>     'Zergling':'Zergling.webp',
>     'Roach':'Roach.webp',
>     'Infestor':'Infestor.webp',
>     'Hydralisk':'Hydralisk.webp',
>     'SwarmHost':'SwarmHost.webp',
>     'MULE':'MULE.webp',
>     'Reaper':'Reaper.webp',
>     'Ghost':'Ghost.webp',
>     'Marauder':'Marauder.webp',
>     'Marine':'Marine.webp',
>     'Battlecruiser':'Battlecruiser.webp',
>     'Thor':'Thor.webp',
>     'Medivac':'Medivac.webp',
>     'Cyclone':'Cyclone.webp',
>     'WidowMine':'WidowMine.webp',
>     'Hellbat':'Hellbat.webp',
>     'Banshee':'Banshee.webp',
>     'Raven':'Raven.webp',
>     'Viking':'Viking.webp',
>     'Liberator':'Liberator.webp',
>     'LiberatorAG':'LiberatorAG.webp',
>     'SiegeTank':'SiegeTank.webp',
>     'SiegeTankSieged':'SiegeTankSieged.webp',
>     'CommandCenter':'CommandCenter.webp',
>     'Nexus':'Nexus.webp',
>     'Pylon':'Pylon.webp',
>     'Assimilator':'Assimilator.webp',
>     'Gateway':'Gateway.webp',
>     'Forge':'Forge.webp',
>     'FleetBeacon':'FleetBeacon.webp',
>     'Barracks':'Barracks.webp',
>     'Factory':'Factory.webp',
>     'Starport':'Starport.webp',
>     'SupplyDepot':'SupplyDepot.webp',
>     'Refinery':'Refinery.webp',
>     'EngineeringBay':'EngineeringBay.webp',
>     'MissileTurret':'MissileTurret.webp',
>     'Bunker':'Bunker.webp',
>     'SensorTower':'SensorTower.webp',
>     'Armory':'Armory.webp',
>     'Assimilator':'Assimilator.webp',
> }
> color_map={'Terran':'#0077cc','Protoss':'#00eaff','Zerg':'#aa3300','Neutral':'#888888'}
> units_output=[]
> inside_units=False
> inside_block=False
> block_lines=[]
> with open('data/data.json','r',encoding='utf-8') as f:
>     for line in f:
>         if '"Unit":[' in line:
>             inside_units=True
>             continue
>         if inside_units:
>             if '"Upgrade":[' in line:
>                 break
>             stripped=line.strip()
>             if stripped.startswith('{'):
>                 inside_block=True
>                 block_lines=[line]
>                 continue
>             if inside_block:
>                 block_lines.append(line)
>                 if stripped.startswith('},'):
>                     block=''.join(block_lines)
>                     m_id=re.search(r'"id":\s*(\d+)', block)
>                     m_name=re.search(r'"name":"([^"\\]+)"', block)
>                     if not m_id or not m_name:
>                         inside_block=False
>                         continue
>                     uid=int(m_id.group(1)); name=m_name.group(1)
>                     if uid in existing_ids:
>                         inside_block=False
>                         continue
>                     # Determine fields
>                     fields=['health']
>                     if '"max_shield":' in block:
>                         fields.append('shields')
>                     if '"max_energy":' in block or '"start_energy":' in block:
>                         fields.append('energy')
>                     if '"weapons":' in block and '"cooldown":' in block:
>                         fields.append('weapon_cooldown')
>                     race_match=re.search(r'"race":"([^"\\]+)"', block)
>                     race=race_match.group(1) if race_match else 'Neutral'
>                     color=color_map.get(race,'#888888')
>                     is_structure='"is_structure":true' in block
>                     is_townhall='"is_townhall":true' in block
>                     size=32.0
>                     if is_structure: size=64.0
>                     if is_townhall: size=96.0
>                     icon=''
>                     # Candidate icon names
>                     candidates=[]
>                     if name in special_map:
>                         candidates.append(special_map[name])
>                     # Add direct name, plus some fallbacks
>                     candidates.extend([
>                         name + '.webp',
>                         name.replace('Terran','Terran').replace('Protoss','Protoss') + '.webp'
>                     ])
>                     # Unique order preserving
>                     seen=set(); cand_final=[]
>                     for c in candidates:
>                         if c not in seen:
>                             cand_final.append(c); seen.add(c)
>                     for c in cand_final:
>                         if c in icons:
>                             icon='icons/'+c
>                             break
>                     fields_toml='[' + ', '.join(f'"{x}"' for x in fields) + ']'
>                     units_output.append(f'[unit.{uid}]\nname = "{name}"\nicon = "{icon}"\nfields = {fields_toml}\nlabel = "{name}"\ncolor = "{color}"\nsize = {size}\n\n')
>                     inside_block=False
> # Print result
> print(''.join(units_output))
