// Sortable list wrapper around @dnd-kit/sortable. Renders children
// inside a `<DndContext>` and a `<SortableContext>`. Children should
// be `<SortableRow id={...}>` elements so the drag handles wire up
// to the dnd-kit hooks.

import { ReactNode } from "react";
import {
  DndContext,
  DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  closestCenter,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

export interface SortableDragHandleProps {
  listeners: ReturnType<typeof useSortable>["listeners"];
  attributes: ReturnType<typeof useSortable>["attributes"];
  setActivatorNodeRef: ReturnType<typeof useSortable>["setActivatorNodeRef"];
  isDragging: boolean;
}

export function SortableList({
  ids,
  onReorder,
  children,
  disabled = false,
}: {
  ids: string[];
  onReorder: (from: number, to: number) => void;
  children: ReactNode;
  disabled?: boolean;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
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
      sensors={disabled ? [] : sensors}
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
  disabled = false,
}: {
  id: string;
  children: (drag: SortableDragHandleProps) => ReactNode;
  disabled?: boolean;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id, disabled });

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
  disabled = false,
}: {
  listeners: ReturnType<typeof useSortable>["listeners"];
  attributes: ReturnType<typeof useSortable>["attributes"];
  setActivatorNodeRef: ReturnType<typeof useSortable>["setActivatorNodeRef"];
  disabled?: boolean;
}) {
  return (
    <button
      ref={setActivatorNodeRef}
      type="button"
      disabled={disabled}
      className="px-1 text-dimmed cursor-grab hover:text-muted active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-30 select-none"
      title={disabled ? "Use PRL order to drag records" : "Drag to reorder"}
      {...(disabled ? {} : attributes)}
      {...(disabled ? {} : listeners)}
      onClick={(event) => event.stopPropagation()}
    >
      ⋮⋮
    </button>
  );
}
