import type { Board } from '../boards/queries';
import { groupByProject } from './boardChoice';

export function BoardPicker({
  boards,
  value,
  onChange,
}: {
  boards: readonly Board[];
  value: string;
  onChange: (boardId: string) => void;
}) {
  return (
    <label className="field db-board-picker">
      <span className="visually-hidden">Board</span>
      <select
        value={value}
        onChange={(event) => {
          onChange(event.target.value);
        }}
      >
        {groupByProject(boards).map((group) => (
          <optgroup key={group.projectKey} label={group.projectKey}>
            {group.boards.map((board) => (
              <option key={board.id} value={board.id}>
                {board.name}
              </option>
            ))}
          </optgroup>
        ))}
      </select>
    </label>
  );
}
