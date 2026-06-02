// Sortable list wrapper around @dnd-kit/sortable. Renders children
// inside a `<DndContext>` and a `<SortableContext>`. Children should
// be `<SortableRow id={...}>` elements so the drag handles wire up
// to the dnd-kit hooks.

import { ReactNode } from "react";
import {
  DndContext,
  DragEndEvent,
  PointerSensor,
  useSensor,
  useSensors,
  closestCenter,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  verticalListSortingStrategy,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

export function SortableList({
  ids,
  onReorder,
  children,
}: {
  ids: string[];
  onReorder: (from: number, to: number) => void;
  children: ReactNode;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } })
  );

  const handleEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    onReorder(from, to);
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleEnd}
    >
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        {children}
      </SortableContext>
    </DndContext>
  );
}

export function SortableRow({
  id,
  children,
}: {
  id: string;
  children: (drag: {
    listeners: ReturnType<typeof useSortable>["listeners"];
    attributes: ReturnType<typeof useSortable>["attributes"];
    setActivatorNodeRef: ReturnType<typeof useSortable>["setActivatorNodeRef"];
    isDragging: boolean;
  }) => ReactNode;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id });

  return (
    <div
      ref={setNodeRef}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0.6 : 1,
      }}
    >
      {children({ listeners, attributes, setActivatorNodeRef, isDragging })}
    </div>
  );
}

export { arrayMove };

export function DragHandle({
  listeners,
  attributes,
  setActivatorNodeRef,
}: {
  listeners: ReturnType<typeof useSortable>["listeners"];
  attributes: ReturnType<typeof useSortable>["attributes"];
  setActivatorNodeRef: ReturnType<typeof useSortable>["setActivatorNodeRef"];
}) {
  return (
    <button
      ref={setActivatorNodeRef}
      type="button"
      className="text-dimmed hover:text-muted px-1 cursor-grab active:cursor-grabbing select-none"
      title="Drag to reorder"
      {...attributes}
      {...listeners}
    >
      ⋮⋮
    </button>
  );
}
