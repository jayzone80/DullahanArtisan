# Rigging, Skinning, IK & Bone Naming Conventions

Concise reference for authoring rigged & animated `.mdl` characters and mechanical armatures in Dullahan Artisan (`artisan`).

---

## 1. Quick DSL Summary

```text
skel
  # World-space rest head -> tail; `m=x` auto-mirrors `_L` -> `_R` / `.L` -> `.R` / `Left` -> `Right`
  bone Hips       -          0,.92,0     0,1.08,0    r=.18
  bone Spine      Hips       0,1.08,0    0,1.32,0    r=.16
  bone UpperLeg_L Hips       .12,.90,0   .12,.48,0   r=.11 m=x
  bone LowerLeg_L UpperLeg_L .12,.48,0   .12,.06,0   r=.09 m=x
  bone Foot_L     LowerLeg_L .12,.06,0   .12,.02,.18 r=.07 m=x
end

# Optional rigid per-part binding (`bone=`) — omit for automatic smooth capsule + Laplacian skinning!
box @visor glass_tint 0,1.65,.12 .20,.08,.04 bone=Head

anim walk dur=0.80 fps=24 loop=1
  key 0.00 Hips dp=0,-0.04,0 r=0,-6,0 i=ease
  # 2-bone analytical IK (`chain=2`) or FABRIK N-bone IK (`chain=3..16`, `tip=1` for robot arms/tails)
  ik  0.00 Foot_L p=.12,0,.22 pole=.12,.5,1.0 chain=2 i=lin
  ev  0.00 sfx:footstep_l
end
```

### Visual Rig & Animation QA Loop
1. **Verify rest bone placement**: `artisan mesh char.mdl --turn out/rig.png --bones` (gold = center/spine, cyan = left, coral = right).
2. **Verify motion arcs & spacing**: `artisan mesh char.mdl --anim walk --paths out/paths.png --track Foot_L,Foot_R,Hips --bones`.
3. **Inspect full action cheaply**: `artisan mesh char.mdl --anim walk --anim-sheet out/sheet.png --frames 8` (compact thumbnail filmstrip with timestamps + `EV:` event badges).

---

## 2. Standard Humanoid Bone Naming Conventions

Match the target engine/retargeting profile when naming bones in `skel ... end`:

| Role | Godot 4 (`SkeletonProfileHumanoid`) | Mixamo (`mixamorig:`) | UE5 / Epic Mannequin | Blender Rigify (`DEF-`) |
| :--- | :--- | :--- | :--- | :--- |
| Root / Pelvis | `Hips` | `mixamorig:Hips` | `pelvis` | `DEF-spine` |
| Lower Spine | `Spine` | `mixamorig:Spine` | `spine_01` | `DEF-spine.001` |
| Upper Spine / Chest | `Chest` / `UpperChest` | `mixamorig:Spine1` / `Spine2` | `spine_03` / `spine_05` | `DEF-spine.002` / `.003` |
| Neck | `Neck` | `mixamorig:Neck` | `neck_01` | `DEF-spine.004` |
| Head | `Head` | `mixamorig:Head` | `head` | `DEF-spine.006` |
| Clavicle / Shoulder | `LeftShoulder` | `mixamorig:LeftShoulder` | `clavicle_l` | `DEF-shoulder.L` |
| Upper Arm | `LeftUpperArm` | `mixamorig:LeftArm` | `upperarm_l` | `DEF-upper_arm.L` |
| Forearm | `LeftLowerArm` | `mixamorig:LeftForeArm` | `lowerarm_l` | `DEF-forearm.L` |
| Hand | `LeftHand` | `mixamorig:LeftHand` | `hand_l` | `DEF-hand.L` |
| Thigh | `LeftUpperLeg` | `mixamorig:LeftUpLeg` | `thigh_l` | `DEF-thigh.L` |
| Shin / Calf | `LeftLowerLeg` | `mixamorig:LeftLeg` | `calf_l` | `DEF-shin.L` |
| Foot | `LeftFoot` | `mixamorig:LeftFoot` | `foot_l` | `DEF-foot.L` |
| Toes | `LeftToes` | `mixamorig:LeftToeBase` | `ball_l` | `DEF-toe.L` |

> **Auto-mirror tip (`m=x`)**: Artisan automatically mirrors `_L`↔`_R`, `_l`↔`_r`, `.L`↔`.R`, and `Left`↔`Right` on both `bone` definitions and `bone=` rigid part bindings when `m=x` is used.

---

## 3. Skinning & IK Craft Tips
- **Organic vs Mechanical**: Leave `bone=` unset on continuous `sdf`/metaball or unified organic bodies so automatic capsule-distance + 3-pass connectivity-smoothed skinning bends knees, elbows, and spines cleanly. Add `bone=<name>` on rigid armor plates, helmets, backpacks, eyes, or robot limbs for zero unwanted squash/shearing.
- **Keep A-Pose or T-Pose clean**: Separate arms slightly from the torso (~30°–45° A-pose or T-pose) and leave a clear gap between thighs in the rest pose so automatic skinning cleanly separates limbs.
- **Pole Vectors**: Always give 2-bone legs a forward knee pole (e.g. `pole=.12,.5,1.0` in `+Z`) and 2-bone arms a backward elbow pole (e.g. `pole=.4,1.3,-1.0` in `-Z`) so knees and elbows never flip backward.
- **Events (`ev`)**: Emit `ev <time> sfx:footstep_l`, `ev <time> hitbox:attack_on`, etc. — exported in standard glTF 2.0 `animation.extras.events` for Three.js (`clip.userData.events`), Godot 4 (`EditorScenePostImport`), and Unity import hooks.

---

## 4. Blend Shapes / Morph Targets (`morph`)

Author facial expressions, breathing, damage dents, or mechanical flaps directly on the rest mesh:

```text
morph blink
  scale @eyes 1,0.12,1
end

morph breathe
  inflate 0,1.26,.04 r=.22 amt=.016
end

morph smile
  pull 0.04,1.52,.13 r=.05 d=0.012,0.018,-0.004 m=x
end

anim wave dur=1.2 fps=24 loop=1
  mw 0.00 blink 0 i=ease
  mw 0.40 blink 1 i=ease
  mw 0.52 blink 0 i=ease
  mw 0.00 breathe 0 i=ease
  mw 0.60 breathe 1 i=ease
  mw 1.20 breathe 0
end
```

- **Exported as native glTF 2.0 morph targets**: `primitive.targets` (`POSITION` + `NORMAL` deltas), `mesh.weights`, `mesh.extras.targetNames` (read natively by Blender Shape Keys, Godot 4 Blend Shapes, Unity SkinnedMeshRenderer, and Three.js `morphTargetDictionary`), plus `weights` animation channels.
- **Deformation order**: Morph targets are evaluated in bind pose *before* skeletal skinning (matching GPU/glTF standard), so blinking/breathing works seamlessly while the head and spine rotate.
- **Quick QA**: `artisan mesh char.mdl --morph blink:1 --render out/blink.png --focus head`.
