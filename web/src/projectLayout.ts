import type { CanvasPlacement, Project } from './types'

export function projectHeight(workerCount: number) {
  return 128 + Math.max(1, Math.ceil(workerCount / 3)) * 108
}

export function nextProjectPlacement(
  projects: Project[],
  workerCount: number,
): CanvasPlacement {
  const width = 350
  const height = projectHeight(workerCount)
  const gap = 52
  const columns = 3

  for (let index = 0; index < Math.max(24, projects.length * 4); index += 1) {
    const candidate = {
      x: 80 + (index % columns) * (width + 120),
      y: 70 + Math.floor(index / columns) * (height + 110),
      width,
      height,
    }
    const overlaps = projects.some((project) => {
      const geometry = project.placement.geometry
      const projectWidth = Math.max(geometry.width, width)
      const projectHeightValue = Math.max(
        geometry.height,
        projectHeight(1),
      )
      return (
        candidate.x < geometry.x + projectWidth + gap &&
        candidate.x + candidate.width + gap > geometry.x &&
        candidate.y < geometry.y + projectHeightValue + gap &&
        candidate.y + candidate.height + gap > geometry.y
      )
    })
    if (!overlaps) return candidate
  }

  return {
    x: 80,
    y:
      Math.max(
        0,
        ...projects.map(
          (project) =>
            project.placement.geometry.y +
            Math.max(project.placement.geometry.height, projectHeight(1)),
        ),
      ) + gap,
    width,
    height,
  }
}
