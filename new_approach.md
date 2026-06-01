# Draw Engine — New Approach

## Draw Objects

- Functions of the draw engine are for **registering and manipulating draw objects**.
- **Draw objects** are things like rectangles, circles, etc.
  - Can draw an outline on the object (e.g. rectangle).
  - Can rotate it.
  - Can translate it.
  - Can change the transform in other ways.
- **Draw object groups**:
  - Have their own transform.
  - Have sub-objects underneath them.
  - The group's transform is also applied to each object in the group.
  - Lets you compose things and rotate/translate them all together, etc.

## Render Approach

- Instead of a `display` call that makes graphic calls to a buffer, use a different approach that is more modular and easier to test.
- Have a **`render_to_draws` function** which returns a list of draw calls that will ultimately be utilized by WGPU.
- Need to choose an appropriate format / representation for the list of draw calls.
