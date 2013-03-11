
import sqlalchemy as _sa

def statement(options, plugin):
    def stmt():
        return str(
            list(
                _sa.create_engine(options["engine"]).execute(
                    _sa.sql.text(options["statement"]),
                    dict(
                        ((str(x), y) for x, y in enumerate(list(plugin.history)))
                    )
                )
            )[0][0]
        )
    return stmt
